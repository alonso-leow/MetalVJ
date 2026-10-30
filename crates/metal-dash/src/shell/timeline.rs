//! The sequence editor's timeline (2026-09-30) — one surface for the song and
//! every model authored over it, replacing the three stacked forms the ticks
//! panel was.
//!
//! Top to bottom, every row sharing one horizontal view (ticks → x):
//!
//! - **ruler** — seconds at the 8 Hz tick rate; click moves the playhead,
//!   drag marks a range (what copy/paste/clear act on);
//! - **golpes** — the song's own hits, diamonds; click toggles one;
//! - **audio** — the digest drawn the way the old studio lane drew it: RMS,
//!   bandwidth and centroid as lines, onset as bars;
//! - **croma** — the twelve pitch classes as a heatmap;
//! - **one row per layer** of the active page (2026-10-01; a page is one
//!   final output — a device or a mask of it). A layer holds clips side by
//!   side: models and video files, each a bar from its mark-in to its
//!   mark-out. A higher layer plays over a lower one, so one model stops
//!   where the next starts — beside it on the same layer, or above/under it.
//!   A clip's edges drag (mark-in/mark-out), its body drags in time and
//!   between layers, and the active clip's keypoint grips retime. The active
//!   clip, expanded, shows one row per float of its schema under its layer;
//! - a last **new layer** row: a video dropped there (or anywhere past the
//!   rows) opens a layer of its own.
//!
//! (The router row is gone, 2026-10-01: the layers say which model holds.)
//!
//! The label column on the left stays put; the rows scroll under it. The
//! mouse wheel scrolls vertically, shift+wheel (or a trackpad's sideways
//! swipe) horizontally, ctrl+wheel zooms about the pointer; both scrollbars
//! drag. Nothing here calls the door — the panel does (regla 6).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use egui::{Align2, Id, Pos2, Rect, RichText, Sense, Shape, Ui, Vec2};
use serde_json::Value;

/// The ruler's tick rate: the canonical storage grid, so a tick means the
/// same thing here as it does in the digest it draws.
const RATE: f32 = metal_schema::TickRate::CANONICAL.hz() as f32;
use crate::perf::Span;
use crate::shell::classes::Theme;
use crate::shell::components::{lucide, Icon};
use crate::shell::lanes::{model_color, IngestLane, TrackView, VIDEO_DEFAULT_TICKS};
use crate::shell::panels::FileDrag;

const LABEL_W: f32 = 150.0;
const RULER_H: f32 = 20.0;
const SONG_H: f32 = 18.0;
const AUDIO_H: f32 = 64.0;
const VOICE_H: f32 = 34.0;
const CHROMA_CELL: f32 = 5.0;
const TRACK_H: f32 = 22.0;
const FLOAT_H: f32 = 20.0;
const EDGE: f32 = 5.0;
const BAR: f32 = 10.0;
const GRIP: f32 = 4.0;
const DOT: f32 = 3.0;
/// Visible ticks, the zoom's two ends.
pub const SPAN_MIN: usize = 16;
pub const SPAN_MAX: usize = 4000;
/// Wheel points per zoom step / per tick of horizontal scroll feel.
const WHEEL_NOTCH: f32 = 50.0;
const ZOOM_STEP: f32 = 1.25;
/// How far ctrl+wheel can shrink or grow the rows.
pub const ROW_ZOOM_MIN: f32 = 0.5;
pub const ROW_ZOOM_MAX: f32 = 4.0;
const BEAT_THRESHOLD: f32 = 0.28;
const LABEL_GAP_PX: f32 = 64.0;

/// One float the expanded track shows, at its offset in the model's row.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub offset: usize,
    pub min: f32,
    pub max: f32,
    /// A colour slot's first colour (three floats), shown as swatches.
    pub color: bool,
}

/// The rows a model's schema `blocks` (`GET /author/models/state`) expand
/// to — the same walk the properties panel's sliders do, so a row and its
/// slider always point at the same float.
pub fn fields_of(blocks: &[Value]) -> Vec<Field> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for block in blocks {
        match block["type"].as_str().unwrap_or_default() {
            "floats" => {
                for f in block["fields"].as_array().into_iter().flatten() {
                    out.push(Field {
                        name: f["name"].as_str().unwrap_or("?").to_string(),
                        offset,
                        min: f["min"].as_f64().unwrap_or(0.0) as f32,
                        max: f["max"].as_f64().unwrap_or(1.0) as f32,
                        color: false,
                    });
                    offset += 1;
                }
            }
            "color_slot" => {
                out.push(Field {
                    name: block["name"].as_str().unwrap_or("color").to_string(),
                    offset,
                    min: 0.0,
                    max: 1.0,
                    color: true,
                });
                offset += 12;
            }
            "camera" => offset += 7,
            _ => {}
        }
    }
    out
}

/// Float `offset`'s own keys among the keypoints: the ticks where it
/// changes, not the ones where only other floats were keyed
/// (`metal_schema::keys`).
pub fn float_track(keyframes: &BTreeMap<usize, Vec<f32>>, offset: usize) -> Vec<(usize, f32)> {
    let keys: Vec<(usize, f32)> = keyframes.iter().filter_map(|(t, f)| Some((*t, *f.get(offset)?))).collect();
    let ticks: Vec<usize> = keys.iter().map(|(t, _)| *t).collect();
    metal_schema::keys::track(&ticks, |k| keys[k].1)
}

#[cfg(test)]
/// Float `offset` at `tick`, eased between its own keys — what
/// [`crate::shell::lanes::interpolate_row`] gives, one float at a time.
pub fn value_at(keyframes: &BTreeMap<usize, Vec<f32>>, tick: usize, offset: usize) -> Option<f32> {
    metal_schema::keys::track_at(&float_track(keyframes, offset), tick as f32)
}

/// `m:ss` for a tick at the ruler's own rate.
pub fn clock(tick: usize) -> String {
    clock_at(tick, RATE)
}

/// [`clock`] at an explicit tick rate — the seam a kittest/test uses to show
/// that a canonical (32 Hz) tick and a base (8 Hz) tick at the same second read
/// the same clock, which is the notation promise the level switch makes.
pub fn clock_at(tick: usize, rate: f32) -> String {
    let s = (tick as f32 / rate) as usize;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The one-glyph face of a blend mode for the layer header's chip — the full
/// name rides in its tooltip.
fn blend_glyph(blend: &str) -> &'static str {
    match blend {
        "add" => "+",
        "multiply" => "×",
        "screen" => "",
        _ => "●",
    }
}

/// A file dropped on the timeline: what, at which tick, onto which layer
/// (`None` — a new one).
#[derive(Debug, Clone)]
pub struct Drop {
    pub file: FileDrag,
    pub tick: usize,
    pub layer: Option<usize>,
}

/// What a frame of [`paint`] asks the panel to do — only what reaches
/// beyond the ingest lane itself.
#[derive(Debug, Default, Clone)]
pub struct TimelineEvents {
    pub dropped: Option<Drop>,
    /// The `+ layer` row was clicked: a layer was added and the panel should
    /// focus the lanes region.
    pub new_layer: bool,
    /// A click on no row at all: empty space in the timeline.
    pub blank: bool,
}

/// A drag in flight on a layer row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Gesture {
    /// The active clip's keypoint from `from` to `to`.
    Retime { from: usize, to: usize },
    /// Clip `i`, grabbed `grab` ticks after its mark-in, to start at `to` on `layer`.
    Move { i: usize, grab: usize, to: usize, layer: usize },
    /// Clip `i`'s mark-in (`start`) or mark-out, to `to`.
    Trim { i: usize, start: bool, to: usize },
}

/// Which row a y in the body is, after the vertical scroll.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Row {
    Song,
    Audio,
    /// The voice block: presence, pitch and the four styles, under the audio
    /// digest. Says "not digested for voice" for a 16-wide matrix instead of
    /// drawing a silent singer (the voice design notes decision 5).
    Voice,
    Chroma,
    /// One layer of the active page.
    Layer(usize),
    /// Clip `i`'s float `k`.
    Float(usize, usize),
    /// The drop zone that opens a new layer.
    NewLayer,
}

/// What a press on a layer row lands on.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    In,
    Out,
    Body,
    Key(usize),
}

/// The rows in paint order with their heights.
fn layout(ingest: &IngestLane, fields: &dyn Fn(&str) -> Vec<Field>) -> Vec<(Row, f32)> {
    // A loop has no ingestion: its digest rows (song, audio, voice, chroma)
    // are not drawn at all.
    let mut rows = if ingest.loop_ticks.is_some() {
        Vec::new()
    } else {
        vec![(Row::Song, SONG_H), (Row::Audio, AUDIO_H), (Row::Voice, VOICE_H), (Row::Chroma, CHROMA_CELL * 12.0)]
    };
    let views = ingest.page_views();
    for layer in ingest.layers() {
        rows.push((Row::Layer(layer), TRACK_H));
        for t in views.iter().filter(|t| t.layer == layer && t.active && t.expanded && t.video.is_none()) {
            for k in 0..fields(t.model).len() {
                rows.push((Row::Float(t.index, k), FLOAT_H));
            }
        }
    }
    rows.push((Row::NewLayer, TRACK_H));
    let zoom = ingest.row_zoom.clamp(ROW_ZOOM_MIN, ROW_ZOOM_MAX);
    rows.into_iter().map(|(r, h)| (r, h * zoom)).collect()
}

/// The song's length in ticks, else the furthest clip end plus a screen.
fn total_ticks(ingest: &IngestLane) -> usize {
    if let Some(n) = ingest.loop_ticks {
        return n.max(1);
    }
    let song = ingest.matrix.as_ref().map(|m| m.ticks).unwrap_or(0);
    let last = ingest.track_views().iter().filter_map(|t| t.bounds.map(|(_, b)| b)).max().unwrap_or(0);
    song.max(last + ingest.span.max(1)).max(1)
}

/// One track's float curves, kept while its keyframes stay the same. The
/// curve of a float is [`float_track`] — an O(keys) fit that the paint used
/// to redo for every float row on every frame.
struct CurveSet {
    keys: BTreeMap<usize, Vec<f32>>,
    /// `float_track(keys, offset)`, computed on first use per offset.
    tracks: Mutex<HashMap<usize, Arc<Vec<(usize, f32)>>>>,
}

impl CurveSet {
    fn track(&self, offset: usize) -> Arc<Vec<(usize, f32)>> {
        let mut m = self.tracks.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(offset).or_insert_with(|| Arc::new(float_track(&self.keys, offset))).clone()
    }
}

/// Every track's [`CurveSet`] by index, kept in egui's temp data between frames.
#[derive(Clone, Default)]
struct CurveCache(Arc<Mutex<HashMap<usize, Arc<CurveSet>>>>);

impl CurveCache {
    fn set_for(&self, index: usize, keys: &BTreeMap<usize, Vec<f32>>) -> Arc<CurveSet> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(set) = m.get(&index).filter(|s| s.keys == *keys) {
            return set.clone();
        }
        let set = Arc::new(CurveSet { keys: keys.clone(), tracks: Mutex::new(HashMap::new()) });
        m.insert(index, set.clone());
        set
    }

    fn retain(&self, shown: &[usize]) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|i, _| shown.contains(i));
    }
}

/// One clip as the gestures see it: index, layer, marks, keypoints (active only).
struct ClipHit {
    i: usize,
    layer: usize,
    a: usize,
    b: usize,
    active: bool,
}

/// Paints the timeline into `height` and applies its gestures to `ingest`.
pub fn paint(
    ui: &mut Ui,
    theme: &Theme,
    ingest: &mut IngestLane,
    fields: &dyn Fn(&str) -> Vec<Field>,
    height: f32,
) -> TimelineEvents {
    let mut events = TimelineEvents::default();
    let _paint = Span::new("timeline.paint");
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    ingest.canvas_rect = Some(rect);
    let id = Id::new("shell/timeline");
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.corner_radius(), theme.inset);

    // Areas: the ruler across the top, the scrollbars along the right and
    // bottom, the label column on the left, the rows in what is left.
    let ruler = Rect::from_min_max(Pos2::new(rect.left() + LABEL_W, rect.top()), Pos2::new(rect.right() - BAR, rect.top() + RULER_H));
    let body = Rect::from_min_max(Pos2::new(rect.left(), ruler.bottom()), Pos2::new(rect.right() - BAR, rect.bottom() - BAR));
    let lanes = Rect::from_min_max(Pos2::new(body.left() + LABEL_W, body.top()), body.max);
    let hbar = Rect::from_min_max(Pos2::new(lanes.left(), body.bottom()), Pos2::new(lanes.right(), rect.bottom()));
    let vbar = Rect::from_min_max(Pos2::new(body.right(), body.top()), Pos2::new(rect.right(), body.bottom()));

    let span = ingest.span.clamp(SPAN_MIN, SPAN_MAX);
    ingest.span = span;
    let total = total_ticks(ingest);
    let _s_layout = Span::new("timeline.layout");
    let rows = layout(ingest, fields);
    drop(_s_layout);
    let content_h: f32 = rows.iter().map(|(_, h)| h).sum();
    let max_scroll_y = (content_h - body.height()).max(0.0);
    let max_from = (total as f32 - span as f32).max(0.0);

    // --- wheel: both axes zoom and pan -------------------------------------
    // wheel or pinch → horizontal zoom, ctrl+wheel → vertical zoom (row
    // heights), shift+wheel → horizontal pan, alt+wheel → vertical pan.
    let hovered = ui.rect_contains_pointer(body.union(ruler));
    if hovered {
        let (delta, mods, pointer) = ui.input_mut(|i| {
            let d = i.smooth_scroll_delta;
            i.smooth_scroll_delta = Vec2::ZERO;
            (d, i.modifiers, i.pointer.hover_pos())
        });
        // ctrl+wheel arrives as egui's zoom gesture, not as a scroll.
        let pinch = ui.input(|i| i.zoom_delta());
        // Some platforms turn shift+wheel into a sideways delta.
        let notch = if delta.y.abs() > delta.x.abs() { delta.y } else { delta.x };
        if mods.command || mods.ctrl {
            let factor = pinch * ZOOM_STEP.powf(notch / WHEEL_NOTCH);
            if factor != 1.0 {
                let anchor_y = pointer.map(|p| p.y - body.top()).unwrap_or(0.0).max(0.0);
                let content_y = (ingest.scroll_y + anchor_y) / ingest.row_zoom;
                ingest.row_zoom = (ingest.row_zoom * factor).clamp(ROW_ZOOM_MIN, ROW_ZOOM_MAX);
                ingest.scroll_y = content_y * ingest.row_zoom - anchor_y;
            }
        } else if mods.alt {
            // At either end the wheel falls through to the parent scroll.
            let at_end = (notch > 0.0 && ingest.scroll_y <= 0.0) || (notch < 0.0 && ingest.scroll_y >= max_scroll_y);
            if at_end {
                ui.input_mut(|i| i.smooth_scroll_delta = delta);
            } else {
                ingest.scroll_y -= notch;
            }
        } else if mods.shift {
            ingest.view_from -= notch / lanes.width() * span as f32;
        } else if pinch != 1.0 || delta.y.abs() > 0.1 {
            let factor = pinch * ZOOM_STEP.powf(delta.y / WHEEL_NOTCH);
            let anchor_x = pointer.map(|p| (p.x - lanes.left()).clamp(0.0, lanes.width())).unwrap_or(0.0);
            let anchor_tick = ingest.view_from + anchor_x / lanes.width() * span as f32;
            let next = ((span as f32 / factor).round() as usize).clamp(SPAN_MIN, SPAN_MAX);
            ingest.span = next;
            ingest.view_from = anchor_tick - anchor_x / lanes.width() * next as f32;
        }
    }
    let span = ingest.span;
    let px = lanes.width() / span as f32;

    // Follow the playhead when it moved off-screen by anything but a scroll.
    let seen = ui.data(|d| d.get_temp::<usize>(id.with("seen")));
    if seen != Some(ingest.playhead) {
        let p = ingest.playhead as f32;
        if p < ingest.view_from || p >= ingest.view_from + span as f32 {
            ingest.view_from = p - span as f32 * 0.25;
        }
        ui.data_mut(|d| d.insert_temp(id.with("seen"), ingest.playhead));
    }

    // --- scrollbars --------------------------------------------------------
    let total_f = (total as f32).max(span as f32);
    let thumb_w = (span as f32 / total_f * hbar.width()).max(BAR * 2.0);
    let free_x = (hbar.width() - thumb_w).max(1.0);
    let h_resp = ui.interact(hbar, id.with("hbar"), Sense::click_and_drag());
    if h_resp.dragged() {
        ingest.view_from += h_resp.drag_delta().x / free_x * max_from;
    } else if h_resp.clicked() {
        if let Some(p) = h_resp.interact_pointer_pos() {
            ingest.view_from = (p.x - hbar.left() - thumb_w * 0.5) / free_x * max_from;
        }
    }
    let thumb_h = (body.height() / content_h.max(1.0) * vbar.height()).clamp(BAR * 2.0, vbar.height());
    let free_y = (vbar.height() - thumb_h).max(1.0);
    let v_resp = ui.interact(vbar, id.with("vbar"), Sense::click_and_drag());
    if v_resp.dragged() {
        ingest.scroll_y += v_resp.drag_delta().y / free_y * max_scroll_y;
    } else if v_resp.clicked() {
        if let Some(p) = v_resp.interact_pointer_pos() {
            ingest.scroll_y = (p.y - vbar.top() - thumb_h * 0.5) / free_y * max_scroll_y;
        }
    }
    ingest.view_from = ingest.view_from.clamp(0.0, max_from);
    ingest.scroll_y = ingest.scroll_y.clamp(0.0, max_scroll_y);
    let from = ingest.view_from;
    let x_of = |tick: f32| lanes.left() + (tick - from) * px;
    let tick_at = |x: f32| (from + (x - lanes.left()) / px).floor().max(0.0) as usize;
    let first = from.floor() as usize;
    let last = (from + span as f32).ceil() as usize + 1;

    // Scrollbar paint.
    painter.rect_filled(hbar, 0.0, theme.panel);
    let hx = hbar.left() + if max_from > 0.0 { from / max_from * free_x } else { 0.0 };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(hx, hbar.top() + 2.0), Vec2::new(thumb_w, hbar.height() - 4.0)),
        theme.corner_radius(),
        if h_resp.hovered() || h_resp.dragged() { theme.ink_weak } else { theme.ink_faint },
    );
    painter.rect_filled(vbar, 0.0, theme.panel);
    let vy = vbar.top() + if max_scroll_y > 0.0 { ingest.scroll_y / max_scroll_y * free_y } else { 0.0 };
    painter.rect_filled(
        Rect::from_min_size(Pos2::new(vbar.left() + 2.0, vy), Vec2::new(vbar.width() - 4.0, thumb_h)),
        theme.corner_radius(),
        if v_resp.hovered() || v_resp.dragged() { theme.ink_weak } else { theme.ink_faint },
    );

    // --- ruler --------------------------------------------------------------
    painter.rect_filled(ruler, 0.0, theme.panel);
    let step = [1usize, 2, 4, 8, 16, 40, 80, 160, 480, 960, 2400]
        .into_iter()
        .find(|s| *s as f32 * px >= LABEL_GAP_PX)
        .unwrap_or(4800);
    let mut t = first / step * step;
    while t <= last {
        let x = x_of(t as f32);
        if x >= ruler.left() && x <= ruler.right() {
            painter.line_segment([Pos2::new(x, ruler.bottom() - 6.0), Pos2::new(x, ruler.bottom())], theme.rule_stroke());
            let label = if step >= RATE as usize { clock(t) } else { t.to_string() };
            painter.text(Pos2::new(x + 3.0, ruler.center().y), Align2::LEFT_CENTER, label, theme.font_small.clone(), theme.ink_weak);
        }
        t += step;
    }
    // A readout of the sequence's tick count, in the label column's top corner.
    // It is an honest zoom readout for a human ("how long is this, in ticks?"),
    // and — because a `Label` carries its text as an AccessKit node — the seam
    // a kittest uses to prove a canonical sequence shows 4× the ticks of a base
    // one at the same zoom, without the ruler's `painter.text` needing a node.
    ui.put(
        Rect::from_min_max(
            Pos2::new(rect.left(), ruler.top()),
            Pos2::new(rect.left() + LABEL_W, ruler.bottom()),
        ),
        egui::Label::new(
            RichText::new(crate::shell::i18n::trf("{n} ticks", &[("n", &total)]))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        )
        .selectable(false),
    );
    let r_resp = ui.interact(ruler, id.with("ruler"), Sense::click_and_drag());
    if let Some(p) = r_resp.interact_pointer_pos() {
        let at = tick_at(p.x);
        if r_resp.drag_started() {
            // Where the press began, not where the drag was recognised.
            let origin = ui.input(|i| i.pointer.press_origin()).map(|o| tick_at(o.x)).unwrap_or(at);
            ui.data_mut(|d| d.insert_temp(id.with("mark-from"), origin));
        }
        if r_resp.dragged() {
            let a = ui.data(|d| d.get_temp::<usize>(id.with("mark-from"))).unwrap_or(at);
            ingest.mark = Some((a.min(at), a.max(at)));
            ingest.seek(a, ui.input(|i| i.time));
        } else if r_resp.clicked() {
            ingest.mark = None;
            ingest.seek(at, ui.input(|i| i.time));
        }
    }

    // --- rows ---------------------------------------------------------------
    struct View {
        index: usize,
        model: String,
        title: String,
        curves: Arc<CurveSet>,
        expanded: bool,
        active: bool,
        layer: usize,
        bounds: Option<(usize, usize)>,
        video: bool,
    }
    let _s_views = Span::new("timeline.views");
    let curve_cache = ui.data(|d| d.get_temp::<CurveCache>(id.with("curves"))).unwrap_or_default();
    let page_tracks = ingest.page_views();
    let curves_of = |t: &TrackView| curve_cache.set_for(t.index, t.keyframes);
    let views: Vec<View> = page_tracks
        .iter()
        .map(|t| View {
            index: t.index,
            model: t.model.to_string(),
            title: t.title(),
            curves: curves_of(t),
            expanded: t.expanded,
            active: t.active,
            layer: t.layer,
            bounds: t.bounds,
            video: t.video.is_some(),
        })
        .collect();
    curve_cache.retain(&views.iter().map(|v| v.index).collect::<Vec<_>>());
    ui.data_mut(|d| d.insert_temp(id.with("curves"), curve_cache.clone()));
    drop(_s_views);
    let active_keys = ingest.keyframes.clone();
    let view_of = |i: usize| views.iter().find(|v| v.index == i);
    let body_painter = ui.painter_at(body);
    // Registered before the rows so a layer header's own controls (added
    // during the loop) win the click over this body-wide catcher — egui
    // gives an overlapping click to the widget added last.
    let b_resp = ui.interact(body, id.with("body"), Sense::click_and_drag());
    let mut gesture = ui.data(|d| d.get_temp::<Option<Gesture>>(id.with("gesture"))).flatten();
    let mut y = body.top() - ingest.scroll_y;
    let mut row_rects = Vec::new();
    let mut clips: Vec<(ClipHit, Rect)> = Vec::new();
    let drag_over = ui.ctx().dragged_id().is_some() && egui::DragAndDrop::has_payload_of_type::<FileDrag>(ui.ctx());
    // The rows' own widgets (the layer title, its mute/solo/blend buttons, the
    // `not digested` note) are clipped to the body: a row scrolled up past the
    // ruler must not keep painting its controls over the tick count above it.
    let outer_clip = ui.clip_rect();
    ui.set_clip_rect(outer_clip.intersect(body));
    for (row, h) in &rows {
        let r = Rect::from_min_size(Pos2::new(body.left(), y), Vec2::new(body.width(), *h));
        y += h;
        // Every row is walked, even one scrolled out of the body: the row
        // widgets must exist in the tree regardless of the body's height, or
        // the structure would change with the theme's spacing (the restyle
        // gate forbids that). Painting stays cheap — `body_painter` clips to
        // the body, so an off-screen row draws nothing visible.
        row_rects.push((*row, r));
        let lane = Rect::from_min_max(Pos2::new(lanes.left(), r.top()), r.max);
        body_painter.line_segment([r.left_bottom(), r.right_bottom()], theme.rule_stroke());
        let label = |text: String, ink| {
            body_painter.text(Pos2::new(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, text, theme.font_small.clone(), ink);
        };
        match *row {
            Row::Song => {
                let _s = Span::new("timeline.song");
                label("golpes".into(), theme.ink_weak);
                for &tick in ingest.song_hits.range(first..last) {
                    let c = Pos2::new(x_of(tick as f32 + 0.5), lane.center().y);
                    let s = (SONG_H * 0.3).min(px.max(3.0));
                    body_painter.add(Shape::convex_polygon(
                        vec![c - Vec2::Y * s, c + Vec2::X * s, c + Vec2::Y * s, c - Vec2::X * s],
                        theme.warn,
                        egui::Stroke::NONE,
                    ));
                }
            }
            Row::Audio => {
                let _s = Span::new("timeline.audio");
                label("audio".into(), theme.ink_weak);
                if let Some(m) = ingest.matrix.as_ref().filter(|m| m.ticks > 0) {
                    let y_of = |v: f32| lane.bottom() - 3.0 - v.clamp(0.0, 1.0) * (lane.height() - 6.0);
                    for t in first..last.min(m.onset.len()) {
                        let h = lane.bottom() - 3.0 - y_of(m.onset[t]);
                        if h > 1.0 {
                            body_painter.rect_filled(
                                Rect::from_min_size(Pos2::new(x_of(t as f32), lane.bottom() - 3.0 - h), Vec2::new((px * 0.5).max(1.0), h)),
                                0.0,
                                theme.phosphor_dim,
                            );
                        }
                    }
                    for (values, color) in [(&m.rms, theme.phosphor), (&m.bandwidth, theme.soft), (&m.centroid, theme.phosphor_lit)] {
                        let pts: Vec<Pos2> = (first..last.min(values.len())).map(|t| Pos2::new(x_of(t as f32 + 0.5), y_of(values[t]))).collect();
                        if pts.len() > 1 {
                            body_painter.add(Shape::line(pts, theme.line_stroke(color)));
                        }
                    }
                } else {
                    body_painter.text(
                        Pos2::new(lane.left() + 8.0, lane.center().y),
                        Align2::LEFT_CENTER,
                        if ingest.waiting_digest { crate::shell::i18n::tr("digesting the song…") } else { crate::shell::i18n::tr("not digested — pick a song") },
                        theme.font_small.clone(),
                        theme.ink_faint,
                    );
                }
            }
            Row::Voice => {
                let _s = Span::new("timeline.voice");
                label(crate::shell::i18n::tr("voice").into(), theme.ink_weak);
                if let Some(m) = ingest.matrix.as_ref() {
                    if m.has_voice {
                        // A presence band along the floor, tinted by the winning
                        // style, then the pitch curve over it. When nothing is
                        // voiced there is simply no bar — never a silent singer.
                        let style_color = [theme.phosphor, theme.warn, theme.soft, theme.soft_dim];
                        let floor = lane.bottom() - 3.0;
                        let range = lane.height() - 6.0;
                        for t in first..last.min(m.voice.len()) {
                            let v = m.voice[t];
                            let presence = v[0].clamp(0.0, 1.0);
                            if presence <= 0.02 {
                                continue;
                            }
                            let style = (0..4)
                                .max_by(|a, b| v[9 + a].total_cmp(&v[9 + b]))
                                .unwrap_or(0);
                            let color = style_color[style];
                            let h = presence * range;
                            body_painter.rect_filled(
                                Rect::from_min_size(Pos2::new(x_of(t as f32), floor - h), Vec2::new(px.max(1.0), h)),
                                0.0,
                                theme.faded(color, (presence * 180.0) as u8),
                            );
                            body_painter.rect_filled(
                                Rect::from_min_size(Pos2::new(x_of(t as f32), lane.top() + 2.0), Vec2::new(px.max(1.0), GRIP)),
                                0.0,
                                color,
                            );
                        }
                        let pts: Vec<Pos2> = (first..last.min(m.voice.len()))
                            .filter(|t| m.voice[*t][0] > 0.02)
                            .map(|t| Pos2::new(x_of(t as f32 + 0.5), floor - m.voice[t][2].clamp(0.0, 1.0) * range))
                            .collect();
                        if pts.len() > 1 {
                            body_painter.add(Shape::line(pts, theme.line_stroke(theme.phosphor_lit)));
                        }
                    } else {
                        // The seam the kittest reads: an AccessKit label saying
                        // the digest has no voice block, rather than an empty row
                        // that looks exactly like a silent singer.
                        ui.put(
                            Rect::from_min_max(Pos2::new(lane.left() + 8.0, lane.top()), lane.max),
                            egui::Label::new(
                                RichText::new(crate::shell::i18n::tr("not digested for voice"))
                                    .font(theme.font_small.clone())
                                    .color(theme.ink_faint),
                            )
                            .selectable(false),
                        );
                    }
                    if m.base_only {
                        // The one honest loss of the migration: an 8 Hz-only
                        // digest held as steps at the fine level.
                        body_painter.text(
                            Pos2::new(lane.right() - 8.0, lane.top() + 8.0),
                            Align2::RIGHT_TOP,
                            crate::shell::i18n::tr("base-only"),
                            theme.font_small.clone(),
                            theme.warn,
                        );
                    }
                }
            }
            Row::Chroma => {
                let _s = Span::new("timeline.chroma");
                label("croma".into(), theme.ink_weak);
                if let Some(m) = ingest.matrix.as_ref() {
                    for t in first..last.min(m.chroma.len()) {
                        for (k, v) in m.chroma[t].iter().enumerate() {
                            if *v < 0.05 {
                                continue;
                            }
                            let cell = Rect::from_min_size(
                                Pos2::new(x_of(t as f32), lane.top() + (11 - k) as f32 * lane.height() / 12.0),
                                Vec2::new(px.max(1.0), lane.height() / 12.0 - 0.5),
                            );
                            body_painter.rect_filled(cell, 0.0, theme.faded(theme.soft, (v.clamp(0.0, 1.0) * 255.0) as u8));
                        }
                    }
                    // A beat the onset suggests, where nothing is authored yet.
                    for t in first.max(1)..last.min(m.onset.len().saturating_sub(1)) {
                        let v = m.onset[t];
                        if v >= BEAT_THRESHOLD && v > m.onset[t - 1] && v >= m.onset[t + 1] && !ingest.keyframes.contains_key(&t) {
                            body_painter.rect_filled(
                                Rect::from_min_size(Pos2::new(x_of(t as f32), lane.bottom() - GRIP), Vec2::new(px.max(2.0), GRIP)),
                                0.0,
                                theme.faded(theme.ink, 56),
                            );
                        }
                    }
                }
            }
            Row::Layer(layer) => {
                let _s = Span::new("timeline.layers");
                let here: Vec<&View> = views.iter().filter(|v| v.layer == layer).collect();
                let active_here = here.iter().find(|v| v.active);
                if active_here.is_some() {
                    body_painter.rect_filled(r, 0.0, theme.faded(theme.phosphor, 24));
                }
                let mark = match active_here {
                    Some(v) if !v.video => if v.expanded { " " } else { " " },
                    _ => "",
                };
                let model_of = active_here
                    .and_then(|v| (!v.model.is_empty()).then_some(v.model.clone()))
                    .or_else(|| here.iter().find_map(|v| (!v.model.is_empty()).then_some(v.model.clone())));
                let name = match model_of {
                    Some(m) => format!("{mark}{} {} · {m}", crate::shell::i18n::tr("layer"), layer + 1),
                    None => format!("{mark}{} {}", crate::shell::i18n::tr("layer"), layer + 1),
                };
                let page = ingest.current_page();
                let mut meta = ingest.layer_meta(&page, layer);
                let mut meta_changed = false;
                let mut focus_layer = false;
                let lab = Rect::from_min_max(Pos2::new(r.left() + 6.0, r.top() + 2.0), Pos2::new(lanes.left() - 6.0, r.bottom() - 2.0));
                // Square chips sized to the row, with a gap between them and
                // an inset from the label column's edge, so they sit inside it.
                let side = (lab.height() - 2.0).clamp(18.0, 26.0);
                let chip = Vec2::splat(side);
                ui.scope_builder(egui::UiBuilder::new().max_rect(lab), |ui| {
                    ui.spacing_mut().item_spacing.x = theme.item_gap.x;
                    ui.spacing_mut().button_padding = theme.field_pad;
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if active_here.is_some() {
                            let blend = meta.blend.clone();
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(blend_glyph(&blend)).font(theme.font_small.clone()))
                                        .min_size(chip),
                                )
                                .on_hover_text(crate::shell::i18n::trf("layer blend · {b}", &[("b", &blend)]))
                                .clicked()
                            {
                                meta.cycle_blend();
                                meta_changed = true;
                            }
                            if ui
                                .add_sized(chip, egui::Button::selectable(meta.solo, RichText::new("S").font(theme.font_small.clone())))
                                .on_hover_text(crate::shell::i18n::tr("solo layer"))
                                .clicked()
                            {
                                meta.solo = !meta.solo;
                                meta_changed = true;
                            }
                            let eye = if meta.muted { Icon::EyeOff } else { Icon::Eye };
                            if ui
                                .add_sized(chip, egui::Button::selectable(meta.muted, lucide(eye, theme.font_small.size)))
                                .on_hover_text(crate::shell::i18n::tr("mute layer"))
                                .clicked()
                            {
                                meta.muted = !meta.muted;
                                meta_changed = true;
                            }
                        }
                        let ink = if active_here.is_some() { theme.ink } else { theme.ink_weak };
                        if ui
                            .add(
                                egui::Label::new(RichText::new(name).font(theme.font_small.clone()).color(ink))
                                    .truncate()
                                    .sense(Sense::click()),
                            )
                            .clicked()
                        {
                            focus_layer = true;
                        }
                    });
                });
                if meta_changed {
                    ingest.set_layer_meta(&page, layer, meta);
                }
                if focus_layer {
                    match active_here {
                        Some(v) => ingest.set_expanded(v.index, !v.expanded),
                        None => {
                            if let Some(v) = here.first() {
                                ingest.select_track(v.index);
                            }
                        }
                    }
                }
                if drag_over && ui.rect_contains_pointer(lane) {
                    body_painter.rect_stroke(lane.shrink(1.0), theme.corner_radius(), theme.accent(), egui::StrokeKind::Inside);
                }
                for v in here {
                    let Some((a, b)) = v.bounds else { continue };
                    let bar = Rect::from_min_max(
                        Pos2::new(x_of(a as f32), lane.top() + 3.0),
                        Pos2::new(x_of(b as f32 + 1.0), lane.bottom() - 3.0),
                    );
                    clips.push((ClipHit { i: v.index, layer, a, b, active: v.active }, bar));
                    let shown = bar.intersect(lane);
                    if shown.width() <= 0.0 {
                        continue;
                    }
                    let color = if v.video {
                        theme.soft
                    } else if v.model.is_empty() {
                        theme.ink_faint
                    } else {
                        model_color(theme, &v.model)
                    };
                    body_painter.rect_filled(shown, theme.corner_radius(), theme.faded(color, 150));
                    if v.active {
                        body_painter.rect_stroke(shown, theme.corner_radius(), theme.accent(), egui::StrokeKind::Inside);
                    }
                    // The mark-in/mark-out handles.
                    for x in [bar.left(), bar.right() - EDGE * 0.5] {
                        let handle = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(EDGE * 0.5, bar.height()));
                        body_painter.rect_filled(handle.intersect(lane), 0.0, theme.ink);
                    }
                    ui.painter_at(shown).text(
                        Pos2::new(shown.left() + EDGE + 2.0, shown.center().y),
                        Align2::LEFT_CENTER,
                        &v.title,
                        theme.font_small.clone(),
                        theme.ink,
                    );
                    if !v.video {
                        for &k in v.curves.keys.range(first..last).map(|(k, _)| k) {
                            let held = v.active && matches!(gesture, Some(Gesture::Retime { from, .. }) if from == k);
                            body_painter.rect_filled(
                                Rect::from_min_size(Pos2::new(x_of(k as f32), lane.bottom() - GRIP - 3.0), Vec2::new(px.max(2.0), GRIP)),
                                0.0,
                                if held { theme.phosphor_lit } else { theme.ink },
                            );
                        }
                    }
                }
            }
            Row::Float(i, k) => {
                let _s = Span::new("timeline.floats");
                let Some(v) = view_of(i) else { continue };
                let fields_here = fields(&v.model);
                let Some(f) = fields_here.get(k) else { continue };
                let curves = v.curves.clone();
                let keys = &curves.keys;
                let active = v.active;
                body_painter.text(Pos2::new(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, &f.name, theme.font_small.clone(), theme.ink_faint);
                let norm = |v: f32| if f.max > f.min { ((v - f.min) / (f.max - f.min)).clamp(0.0, 1.0) } else { 0.0 };
                let y_of = |v: f32| lane.bottom() - 3.0 - norm(v) * (lane.height() - 6.0);
                // Marks only where this float is keyed, not at every keypoint
                // of the clip (the other floats' keys pass through it). The
                // curves are the track's own, kept across frames.
                let own_tracks: Vec<Arc<Vec<(usize, f32)>>> = (f.offset..f.offset + if f.color { 12 } else { 1 }).map(|o| curves.track(o)).collect();
                let own = |t: usize| own_tracks.iter().any(|tr| tr.binary_search_by_key(&t, |(k, _)| *k).is_ok());
                if f.color {
                    for (&t, row) in keys.range(first..last).filter(|(t, _)| own(**t)) {
                        if let (Some(r0), Some(g0), Some(b0)) = (row.get(f.offset), row.get(f.offset + 1), row.get(f.offset + 2)) {
                            let sw = Rect::from_center_size(Pos2::new(x_of(t as f32 + 0.5), lane.center().y), Vec2::splat(FLOAT_H - 6.0));
                            body_painter.rect_filled(sw, 0.0, theme.color_of([*r0, *g0, *b0]));
                        }
                    }
                } else if !keys.is_empty() {
                    let track = &own_tracks[0];
                    let pts: Vec<Pos2> = (first..last)
                        .filter_map(|t| metal_schema::keys::track_at(track, t as f32).map(|v| Pos2::new(x_of(t as f32 + 0.5), y_of(v))))
                        .collect();
                    if pts.len() > 1 {
                        body_painter.add(Shape::line(pts, theme.line_stroke(if active { theme.phosphor } else { theme.ink_faint })));
                    }
                    for (&t, row) in keys.range(first..last).filter(|(t, _)| own(**t)) {
                        if let Some(v) = row.get(f.offset) {
                            body_painter.circle_filled(Pos2::new(x_of(t as f32 + 0.5), y_of(*v)), DOT, if active { theme.phosphor_lit } else { theme.ink_weak });
                        }
                    }
                }
            }
            Row::NewLayer => {
                label(format!("+ {}", crate::shell::i18n::tr("layer")).into(), theme.ink_faint);
                body_painter.text(
                    Pos2::new(lane.left() + 8.0, lane.center().y),
                    Align2::LEFT_CENTER,
                    crate::shell::i18n::tr("drop a video from the library: onto a layer, or here for a new one"),
                    theme.font_small.clone(),
                    theme.ink_faint,
                );
                if drag_over && ui.rect_contains_pointer(lane) {
                    body_painter.rect_stroke(lane.shrink(1.0), theme.corner_radius(), theme.accent(), egui::StrokeKind::Inside);
                }
            }
        }
    }
    ui.set_clip_rect(outer_clip);
    // The label column's divider.
    body_painter.line_segment([Pos2::new(lanes.left(), body.top()), Pos2::new(lanes.left(), body.bottom())], theme.rule_bright_stroke());

    // --- the marked range and the playhead, over every row -----------------
    let full = Rect::from_min_max(Pos2::new(lanes.left(), ruler.top()), lanes.max);
    let over = ui.painter_at(full);
    if let Some((a, b)) = ingest.mark {
        over.rect_filled(
            Rect::from_min_max(Pos2::new(x_of(a as f32), full.top()), Pos2::new(x_of(b as f32 + 1.0), full.bottom())),
            0.0,
            theme.faded(theme.soft, 36),
        );
    }
    let ph = x_of(ingest.playhead as f32);
    over.line_segment([Pos2::new(ph, full.top()), Pos2::new(ph, full.bottom())], theme.accent());
    match gesture {
        Some(Gesture::Retime { to, .. } | Gesture::Trim { to, .. }) => {
            let x = x_of(to as f32);
            over.line_segment([Pos2::new(x, full.top()), Pos2::new(x, full.bottom())], theme.line_stroke(theme.phosphor_lit));
        }
        Some(Gesture::Move { i, to, layer, .. }) => {
            let len = clips.iter().find(|(c, _)| c.i == i).map(|(c, _)| c.b - c.a).unwrap_or(0);
            if let Some((_, r)) = row_rects.iter().find(|(row, _)| *row == Row::Layer(layer)) {
                let ghost = Rect::from_min_max(
                    Pos2::new(x_of(to as f32), r.top() + 2.0),
                    Pos2::new(x_of((to + len) as f32 + 1.0), r.bottom() - 2.0),
                );
                ui.painter_at(lanes).rect_stroke(ghost, theme.corner_radius(), theme.line_stroke(theme.phosphor_lit), egui::StrokeKind::Inside);
            }
        }
        None => {}
    }

    let _s_gest = Span::new("timeline.gestures");
    // --- gestures on the rows ------------------------------------------------
    let pointer = b_resp.interact_pointer_pos().or(b_resp.hover_pos());
    let row_at = |p: Pos2| row_rects.iter().find(|(_, r)| r.contains(p)).map(|(row, _)| *row);
    // What a press at `p` lands on: a clip's edge, the active clip's
    // keypoint grip, or a clip's body — the topmost-painted clip first.
    let hit = |p: Pos2| -> Option<(&ClipHit, Part)> {
        clips.iter().rev().find_map(|(c, bar)| {
            if p.y < bar.top() || p.y > bar.bottom() || p.x < bar.left() - EDGE || p.x > bar.right() + EDGE {
                return None;
            }
            if (p.x - bar.left()).abs() <= EDGE {
                return Some((c, Part::In));
            }
            if (p.x - bar.right()).abs() <= EDGE {
                return Some((c, Part::Out));
            }
            if p.x < bar.left() || p.x > bar.right() {
                return None;
            }
            if c.active && p.y >= bar.bottom() - GRIP - 4.0 {
                let tick = tick_at(p.x);
                if let Some(k) = active_keys.range(tick.saturating_sub(1)..=tick + 1).map(|(k, _)| *k).min_by_key(|k| k.abs_diff(tick)) {
                    return Some((c, Part::Key(k)));
                }
            }
            Some((c, Part::Body))
        })
    };
    if let Some(p) = pointer {
        let tick = tick_at(p.x);
        let in_labels = p.x < lanes.left();
        if b_resp.hovered() && !in_labels && gesture.is_none() {
            match hit(p).map(|(_, part)| part) {
                Some(Part::In | Part::Out) => ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal),
                Some(Part::Body) => ui.ctx().set_cursor_icon(egui::CursorIcon::Grab),
                _ => {}
            }
        }
        if b_resp.drag_started() && !in_labels {
            let p = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            let tick = tick_at(p.x);
            gesture = hit(p).map(|(c, part)| match part {
                Part::Key(k) => Gesture::Retime { from: k, to: k },
                Part::In => Gesture::Trim { i: c.i, start: true, to: c.a },
                Part::Out => Gesture::Trim { i: c.i, start: false, to: c.b },
                Part::Body => Gesture::Move { i: c.i, grab: tick.saturating_sub(c.a), to: c.a, layer: c.layer },
            });
        }
        if b_resp.dragged() {
            match gesture.as_mut() {
                Some(Gesture::Retime { to, .. } | Gesture::Trim { to, .. }) => *to = tick,
                Some(Gesture::Move { grab, to, layer, .. }) => {
                    *to = tick.saturating_sub(*grab);
                    if let Some(Row::Layer(l)) = row_at(p) {
                        *layer = l;
                    }
                }
                None => {}
            }
        }
        if b_resp.drag_stopped() {
            match gesture.take() {
                Some(Gesture::Retime { from, to }) => {
                    ingest.move_keyframe(from, to);
                }
                Some(Gesture::Trim { i, start, to }) => {
                    ingest.trim_clip(i, start, to);
                    ingest.select_track(i);
                }
                Some(Gesture::Move { i, to, layer, .. }) => {
                    ingest.move_clip(i, to, layer);
                    ingest.select_track(i);
                }
                None => {}
            }
        }
        if b_resp.clicked() {
            match row_at(p) {
                Some(Row::Song) if !in_labels => {
                    ingest.seek(tick, ui.input(|i| i.time));
                    ingest.toggle_song_hit(tick);
                }
                Some(Row::Layer(layer)) if in_labels => {
                    // The label: select the layer's first clip, or fold the
                    // active one's floats.
                    match views.iter().find(|v| v.layer == layer && v.active) {
                        Some(v) => ingest.set_expanded(v.index, !v.expanded),
                        None => {
                            if let Some(v) = views.iter().find(|v| v.layer == layer) {
                                ingest.select_track(v.index);
                            }
                        }
                    }
                }
                Some(Row::Layer(_)) => {
                    if let Some((c, _)) = hit(p) {
                        ingest.select_track(c.i);
                    }
                    ingest.seek(tick, ui.input(|i| i.time));
                }
                Some(Row::Float(i, _)) => {
                    ingest.select_track(i);
                    if !in_labels {
                        ingest.seek(tick, ui.input(|i| i.time));
                    }
                }
                // `+ layer`: a click anywhere on the row opens a new layer
                // and asks the panel to focus the lanes region.
                Some(Row::NewLayer) => {
                    ingest.add_track();
                    events.new_layer = true;
                }
                Some(_) if !in_labels => ingest.seek(tick, ui.input(|i| i.time)),
                None => events.blank = true,
                _ => {}
            }
        }
        if let Some(file) = b_resp.dnd_release_payload::<FileDrag>() {
            let layer = match row_at(p) {
                Some(Row::Layer(l)) => Some(l),
                Some(Row::Float(i, _)) => view_of(i).map(|v| v.layer),
                _ => None,
            };
            events.dropped = Some(Drop { file: (*file).clone(), tick: if in_labels { ingest.playhead } else { tick }, layer });
        }
    }
    if !b_resp.dragged() && !b_resp.drag_stopped() {
        gesture = None;
    }
    ui.data_mut(|d| d.insert_temp(id.with("gesture"), gesture));
    // The labels' `ui.put`s leave the layout cursor under the ruler; put it
    // back under the whole timeline so what follows (the bottom bar) is laid
    // out below the rows instead of over the song and audio rows.
    ui.advance_cursor_after_rect(rect);
    events
}

/// How many ticks a dropped video spans: its length when the library knows
/// it, else [`VIDEO_DEFAULT_TICKS`].
pub fn video_ticks(seconds: Option<f64>) -> usize {
    seconds.filter(|s| *s > 0.0).map_or(VIDEO_DEFAULT_TICKS, |s| (s * f64::from(RATE)).ceil() as usize)
}

/// The song hits as the set the panel saves.
pub fn hits_body(song: &str, hits: &BTreeSet<usize>) -> Value {
    serde_json::json!({ "song": song, "ticks": hits.iter().collect::<Vec<_>>() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A loop has no ingestion: its layout carries no digest row, only the
    /// layers; an ordinary sequence keeps all four digest rows.
    #[test]
    fn a_loop_draws_no_digest_rows() {
        let no_fields = |_: &str| Vec::new();
        let digest = |r: &Row| matches!(r, Row::Song | Row::Audio | Row::Voice | Row::Chroma);
        let mut lp = IngestLane::default();
        lp.start_loop(8);
        assert!(!layout(&lp, &no_fields).iter().any(|(r, _)| digest(r)));
        assert_eq!(total_ticks(&lp), 8, "the loop's timeline is its own length");
        let seq = IngestLane::default();
        assert_eq!(layout(&seq, &no_fields).iter().filter(|(r, _)| digest(r)).count(), 4);
    }

    /// A row is the same float its slider is: offsets walk floats, colour
    /// slots (12) and cameras (7) exactly as the properties panel does.
    #[test]
    fn fields_follow_the_schema_offsets() {
        let blocks = vec![
            json!({ "type": "floats", "fields": [{ "name": "a", "min": 0.0, "max": 2.0 }, { "name": "b" }] }),
            json!({ "type": "color_slot", "name": "palette" }),
            json!({ "type": "camera" }),
            json!({ "type": "floats", "fields": [{ "name": "c" }] }),
        ];
        let f = fields_of(&blocks);
        let at: Vec<(&str, usize)> = f.iter().map(|f| (f.name.as_str(), f.offset)).collect();
        assert_eq!(at, vec![("a", 0), ("b", 1), ("palette", 2), ("c", 21)]);
        assert!(f[2].color);
        assert_eq!(f[0].max, 2.0);
    }

    /// One float, eased between keypoints, flat past either end — the same
    /// numbers `interpolate_row` gives for the whole row.
    #[test]
    fn value_at_agrees_with_the_row_interpolation() {
        let mut keys = BTreeMap::new();
        keys.insert(2, vec![0.0, 1.0]);
        keys.insert(10, vec![1.0, 0.0]);
        for t in 0..14 {
            let row = crate::shell::lanes::interpolate_row(&keys, t).unwrap();
            assert!((value_at(&keys, t, 0).unwrap() - row[0]).abs() < 1e-6, "tick {t}");
            assert!((value_at(&keys, t, 1).unwrap() - row[1]).abs() < 1e-6, "tick {t}");
        }
        assert_eq!(value_at(&BTreeMap::new(), 3, 0), None);
    }

    #[test]
    fn the_clock_is_minutes_and_seconds_at_the_ruler_rate() {
        assert_eq!(clock(0), "0:00");
        // The ruler is canonical now: 32 ticks to the second.
        assert_eq!(clock(32 * 75), "1:15");
    }

    /// A canonical tick and a base tick at the same second read the same clock —
    /// the notation does not move with the level.
    #[test]
    fn the_clock_agrees_across_levels_at_the_same_second() {
        let base_ticks = 8 * 60; // a minute on the base grid
        let canon_ticks = metal_schema::TickRate::Base.to_canonical(base_ticks as i64) as usize;
        assert_eq!(canon_ticks, base_ticks * 4, "canonical is exactly 4× the base");
        assert_eq!(clock_at(base_ticks, 8.0), clock_at(canon_ticks, 32.0));
        assert_eq!(clock_at(canon_ticks, 32.0), "1:00");
    }

    /// A track's float curves are reused while its keyframes are unchanged,
    /// rebuilt when one changes, and dropped once the track is not shown —
    /// and the curve handed out is always `float_track`'s own.
    #[test]
    fn curves_are_reused_until_the_keys_change() {
        let cache = CurveCache::default();
        let mut keys = BTreeMap::new();
        for t in (0..40).step_by(4) {
            keys.insert(t, vec![(t as f32 * 0.1).sin(), 0.5]);
        }
        let first = cache.set_for(3, &keys);
        assert!(Arc::ptr_eq(&first, &cache.set_for(3, &keys)), "the same keys reuse the set");
        assert_eq!(*first.track(0), float_track(&keys, 0));
        keys.insert(40, vec![1.0, 0.5]);
        let changed = cache.set_for(3, &keys);
        assert!(!Arc::ptr_eq(&first, &changed), "a new keypoint rebuilds the set");
        assert_eq!(*changed.track(0), float_track(&keys, 0));
        cache.retain(&[]);
        assert!(cache.0.lock().unwrap().is_empty(), "a track no longer shown is dropped");
    }

    /// A scene card released on the timeline body lands at the pointer's tick:
    /// a fresh lane's playhead is at 0, so a drop out on the body keeps a
    /// later tick of its own rather than the playhead.
    #[test]
    fn a_drop_on_the_timeline_body_lands_at_the_pointer_tick() {
        use egui_kittest::kittest::Queryable;
        let theme = Theme::phosphor();
        let sink: std::rc::Rc<std::cell::RefCell<Option<usize>>> = Default::default();
        let area: std::rc::Rc<std::cell::RefCell<Option<Rect>>> = Default::default();
        let (out, body_at) = (sink.clone(), area.clone());
        let mut ingest = IngestLane::default();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            let card = ui.add(egui::Label::new("scene card").sense(Sense::click_and_drag()));
            card.dnd_set_drag_payload(FileDrag {
                name: "scene card".to_string(),
                path: "/clips/a.mp4".to_string(),
                kind: "video".to_string(),
                seconds: Some(3.0),
                scene: None,
            });
            let body = ui.available_rect_before_wrap();
            *body_at.borrow_mut() = Some(body);
            let painted = ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                paint(ui, &theme, &mut ingest, &|_| Vec::new(), body.height())
            });
            if let Some(drop) = painted.inner.dropped {
                *out.borrow_mut() = Some(drop.tick);
            }
        });
        harness.run();
        let from = harness.get_by_label("scene card").rect().center();
        let to = area.borrow().expect("the timeline painted").center();
        harness.drag_at(from);
        harness.run();
        harness.hover_at(to);
        harness.run();
        harness.drop_at(to);
        harness.run();
        let tick = sink.borrow().expect("a release on the body is a drop");
        assert!(tick > 0, "the drop lands past the playhead at 0, got tick {tick}");
    }
}
