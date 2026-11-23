//! The persistent transport bar (plan W6): one strip under every shell view,
//! Edit, Show, Timeline, Monitor and Show + monitor alike.
//!
//! Centre: prev keypoint · the big play/pause disc · next keypoint · stop,
//! with the scrubber under them (elapsed clock, seek bar, total). Left: the
//! sequence's title. Right: the loop toggle. The phone's compact form keeps
//! only the disc and the scrubber.
//!
//! It drives the sequence editor's own playback ([`IngestLane`]) through the
//! same methods the Space key and the lanes panel call, so there is one clock.
//! The loop toggle posts `POST /loop/audio`, the route the Looper's readout
//! already reads. Nothing here mutates [`super::Shell`] beyond the lane it is
//! handed.

use egui::{Align, Align2, Layout, Pos2, Rect, RichText, Sense, Ui, Vec2, WidgetInfo, WidgetType};
use serde_json::json;

use crate::call::Call;

use super::classes::{glyph_font, Theme};
use super::components::{glyph, Icon};
use super::i18n::tr;
use super::lanes::IngestLane;
use super::timeline;

/// The playhead tick at `fraction` (0.0 = the first tick, 1.0 = the end) of a
/// sequence `total` ticks long. Clamped to `0..total`; an empty sequence is
/// tick 0.
pub(super) fn tick_at(fraction: f32, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let f = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
    ((f * total as f32).floor() as usize).min(total - 1)
}

/// The inverse of [`tick_at`]: where `tick` sits along a seek bar of a
/// sequence `total` ticks long, in `0.0..=1.0`.
pub(super) fn fraction_of(tick: usize, total: usize) -> f32 {
    if total == 0 {
        return 0.0;
    }
    (tick.min(total) as f32 / total as f32).clamp(0.0, 1.0)
}

/// The fraction of a seek bar a pointer at `x` picks, given the bar's
/// `left` edge and `width`. A zero-width bar picks 0.
pub(super) fn pointer_fraction(x: f32, left: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    ((x - left) / width).clamp(0.0, 1.0)
}

/// Paints the bar's contents (the caller wraps it in the bottom `Panel` that
/// gives it its height). `compact` is the phone arrangement.
pub(super) fn paint(ui: &mut Ui, theme: &Theme, door: &dyn Call, ingest: &mut IngestLane, compact: bool) {
    let now = ui.input(|i| i.time);
    if compact {
        ui.vertical_centered(|ui| {
            play_row(ui, theme, ingest, now);
            scrubber(ui, theme, ingest, now);
        });
        return;
    }
    ui.columns(3, |cols| {
        title(&mut cols[0], theme, ingest);
        cols[1].vertical_centered(|ui| {
            play_row(ui, theme, ingest, now);
            scrubber(ui, theme, ingest, now);
        });
        cols[2].with_layout(Layout::right_to_left(Align::Center), |ui| {
            loop_toggle(ui, theme, door, ingest);
        });
    });
}

/// The sequence's name, left of the controls; a placeholder when none is loaded.
fn title(ui: &mut Ui, theme: &Theme, ingest: &IngestLane) {
    let text = if ingest.song.is_empty() { tr("no sequence loaded").to_string() } else { ingest.song.clone() };
    ui.add(egui::Label::new(RichText::new(text).font(theme.font_body.clone()).color(theme.ink)).truncate());
}

/// The control row: prev · play/pause · next · stop, in tight square cells.
fn play_row(ui: &mut Ui, theme: &Theme, ingest: &mut IngestLane, now: f64) {
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing = theme.transport_cell_gap;
        let shift = ui.input(|i| i.modifiers.shift);
        let total = timeline::total_ticks(ingest);
        if cell(ui, theme, Icon::StepBack, tr("previous frame (shift: keypoint)"), false).clicked() {
            let to = if shift {
                ingest.neighbour_keyframe(false)
            } else {
                Some(ingest.playhead.saturating_sub(1))
            };
            if let Some(t) = to {
                ingest.goto(t);
            }
        }
        let playing = ingest.playing.is_some();
        let (icon, tip) = if playing { (Icon::Pause, tr("pause")) } else { (Icon::Play, tr("play")) };
        if play_disc(ui, theme, icon, tip).clicked() {
            ingest.toggle_play(now);
        }
        if cell(ui, theme, Icon::StepForward, tr("next frame (shift: keypoint)"), false).clicked() {
            let to = if shift {
                ingest.neighbour_keyframe(true)
            } else {
                Some((ingest.playhead + 1).min(total.saturating_sub(1)))
            };
            if let Some(t) = to {
                ingest.goto(t);
            }
        }
        if cell(ui, theme, Icon::Stop, tr("stop"), false).clicked() {
            ingest.playing = None;
            ingest.goto(0);
        }
    });
}

/// The scrubber: the elapsed clock, the seek bar, the total time.
fn scrubber(ui: &mut Ui, theme: &Theme, ingest: &mut IngestLane, now: f64) {
    let total = timeline::total_ticks(ingest);
    let elapsed = timeline::clock(ingest.playhead);
    let end = timeline::clock(total);
    let font = theme.font_small.clone();
    let text_w = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), theme.ink_weak).size().x;
    let gap = ui.spacing().item_spacing.x;
    let bar_w = (ui.available_width() - text_w(&elapsed) - text_w(&end) - gap * 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&elapsed).font(font.clone()).color(theme.ink_weak));
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(bar_w, theme.interact_size.y), Sense::click_and_drag());
        seek_bar(ui, theme, ingest, total, now, rect, &resp);
        ui.label(RichText::new(&end).font(font.clone()).color(theme.ink_weak));
    });
}

/// Paints the seek track, the filled part up to the playhead, and a thumb at
/// the pointer while hovered (else at the playhead). A click or drag seeks.
fn seek_bar(ui: &mut Ui, theme: &Theme, ingest: &mut IngestLane, total: usize, now: f64, rect: Rect, resp: &egui::Response) {
    let mid = rect.center().y;
    let left = rect.left();
    let width = rect.width();
    let x_of = |f: f32| left + width * f;
    let painter = ui.painter_at(rect);
    painter.line_segment([Pos2::new(left, mid), Pos2::new(rect.right(), mid)], theme.rule_stroke());
    let head = fraction_of(ingest.playhead, total);
    painter.line_segment([Pos2::new(left, mid), Pos2::new(x_of(head), mid)], theme.accent());
    let hover = resp.hover_pos().filter(|_| resp.hovered() || resp.dragged());
    if let Some(p) = hover {
        let at = pointer_fraction(p.x, left, width);
        let r = rect.height() * 0.5;
        painter.circle_stroke(Pos2::new(x_of(at), mid), r, theme.accent());
    } else {
        painter.circle_filled(Pos2::new(x_of(head), mid), rect.height() * 0.25, theme.phosphor);
    }
    if resp.clicked() || resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let f = pointer_fraction(p.x, left, width);
            ingest.seek(tick_at(f, total), now);
        }
    }
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Slider, true, "seek"));
}

/// The loop toggle: `POST /loop/audio` with `exit` when a loop is engaged,
/// `lock` (engage the detector's current length) when not. A refusal lands in
/// the lane's note, the same way the lanes panel reports its own.
fn loop_toggle(ui: &mut Ui, theme: &Theme, door: &dyn Call, ingest: &mut IngestLane) {
    let engaged = door
        .json("GET", "/state", None)
        .ok()
        .and_then(|s| s["repeat"]["engaged"].as_bool())
        .unwrap_or(false);
    let colour = if engaged { theme.phosphor_lit } else { theme.ink_weak };
    let body = if engaged { json!({ "exit": true }) } else { json!({ "lock": true }) };
    if cell_coloured(ui, theme, Icon::Loop, tr("loop"), colour).clicked() {
        if let Err((_, msg)) = door.json("POST", "/loop/audio", Some(body)) {
            ingest.note = Some(msg);
        }
    }
}

/// One square cell with a glyph; the colour lights when `on`.
fn cell(ui: &mut Ui, theme: &Theme, icon: Icon, tip: &str, on: bool) -> egui::Response {
    let colour = if on { theme.phosphor_lit } else { theme.ink };
    cell_coloured(ui, theme, icon, tip, colour)
}

fn cell_coloured(ui: &mut Ui, theme: &Theme, icon: Icon, tip: &str, colour: egui::Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(theme.transport_cell_size, Sense::click());
    let resp = resp.on_hover_text(tip);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, tip));
    if ui.is_rect_visible(rect) {
        let colour = if resp.hovered() { theme.phosphor_lit } else { colour };
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph(icon), glyph_font(theme.font_body.size), colour);
    }
    resp
}

/// The big accent-filled disc with the play or pause glyph.
fn play_disc(ui: &mut Ui, theme: &Theme, icon: Icon, tip: &str) -> egui::Response {
    let size = Vec2::splat(theme.transport_play_size);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let resp = resp.on_hover_text(tip);
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, tip));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.circle_filled(rect.center(), rect.height() * 0.5, theme.phosphor);
        painter.text(rect.center(), Align2::CENTER_CENTER, glyph(icon), glyph_font(theme.font_body.size), theme.panel);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::{fraction_of, pointer_fraction, tick_at};

    #[test]
    fn a_seek_bar_position_maps_to_a_tick_and_back() {
        assert_eq!(tick_at(0.0, 100), 0);
        assert_eq!(tick_at(0.5, 100), 50);
        assert_eq!(tick_at(1.0, 100), 99, "the far end is the last tick, never past it");
        assert_eq!(tick_at(-3.0, 100), 0, "a pointer left of the bar clamps to the start");
        assert_eq!(tick_at(7.0, 100), 99, "and right of it to the end");
        assert_eq!(tick_at(f32::NAN, 100), 0);
        assert_eq!(tick_at(0.9, 0), 0, "an empty sequence has one place: tick 0");
        for t in [0usize, 1, 37, 99] {
            assert_eq!(tick_at(fraction_of(t, 100), 100), t, "tick {t} round-trips");
        }
        assert_eq!(fraction_of(250, 100), 1.0, "past the end reads as the full bar");
        assert_eq!(fraction_of(5, 0), 0.0);
    }

    #[test]
    fn the_pointer_fraction_is_relative_to_the_bar() {
        assert_eq!(pointer_fraction(110.0, 100.0, 200.0), 0.05);
        assert_eq!(pointer_fraction(50.0, 100.0, 200.0), 0.0);
        assert_eq!(pointer_fraction(400.0, 100.0, 200.0), 1.0);
        assert_eq!(pointer_fraction(150.0, 100.0, 0.0), 0.0, "a collapsed bar picks the start");
    }
}
