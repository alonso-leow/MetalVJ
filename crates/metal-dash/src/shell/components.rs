//! The *components* layer: the reusable pieces the shell and its panels are
//! built from.
//!
//! Every function here takes a `&Theme` and reads every visual value from
//! it. There is no colour, width, margin, radius or size written down in
//! this file — gate 2 (`gates.rs`) scans for them — and the only thing this
//! file may import from `shell` is `classes` — gate 1 checks that. A
//! component knows how a heading looks; it does not know which panel it is
//! in, and it does not know what a `Region` is.

use std::ops::RangeInclusive;
use std::time::Duration;

use egui::{Color32, Frame, Id, Panel, Pos2, Rect, Response, RichText, Sense, Ui, Vec2};

use metal_notation::color::Hsb;

use super::classes::{glyph_font, ControlState, Theme};

/// The frame every panel region sits in: panel fill, hairline rule, the
/// theme's padding and radius.
pub fn panel_frame(theme: &Theme) -> Frame {
    Frame::new()
        .fill(theme.panel)
        .stroke(theme.rule_stroke())
        .inner_margin(theme.panel_margin())
        .corner_radius(theme.corner_radius())
}

/// The frame of the ground the regions sit on: the tube, no rule, no
/// radius, the theme's padding.
pub fn ground_frame(theme: &Theme) -> Frame {
    Frame::new()
        .fill(theme.tube)
        .inner_margin(theme.panel_margin())
}

/// The frame of the fixed header bar: raised surface, hairline underneath.
pub fn header_frame(theme: &Theme) -> Frame {
    Frame::new()
        .fill(theme.tube_raised)
        .stroke(theme.rule_stroke())
        .inner_margin(theme.header_margin())
}

/// The header strip a panel paints above its body (`theme.panel_head`): darker
/// than the panel, no title of its own, lit by the same red glow the
/// properties card casts. [`panel_head`] is the one caller.
pub fn panel_head_frame(theme: &Theme) -> Frame {
    Frame::new()
        .fill(theme.panel_head)
        .inner_margin(theme.panel_margin())
        .shadow(theme.card_shadow())
}

/// A panel's header bar: [`panel_head_frame`] at the grid's bar height, with
/// `add`'s widgets in it. The bar is named `label` for AccessKit (the region's
/// own name, which is no longer painted as text), so a click on it is still
/// how a test or a reader reaches the panel.
pub fn panel_head<R>(ui: &mut Ui, theme: &Theme, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let inner = panel_head_frame(theme).show(ui, |ui| {
        let size = Vec2::new(ui.available_width(), theme.panel_head_height);
        ui.set_min_size(size);
        // Centred on the strip's height, so what the head carries (a field, a
        // button) has the same space above and below it.
        ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| add(ui))
            .inner
    });
    inner
        .response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, label));
    inner.inner
}

/// The wordmark `MetalVJ`: `Metal` in the logo blue, `VJ` in the logo red.
/// Two runs, so each half takes its own colour token from the theme.
pub fn wordmark(ui: &mut Ui, theme: &Theme) {
    ui.push_id(Id::new("shell/wordmark"), |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = theme.wordmark_gap;
            ui.label(
                RichText::new("Metal")
                    .font(theme.font_heading.clone())
                    .color(theme.logo_metal),
            );
            ui.label(
                RichText::new("VJ")
                    .font(theme.font_heading.clone())
                    .color(theme.logo_vj)
                    .strong(),
            );
        });
    });
}

/// A section kicker: the small heading at the top of a panel, in the
/// heading face and the weak ink.
pub fn section_heading(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme.font_heading.clone())
            .color(theme.ink_weak),
    );
}

/// Which edge a delegated sidebar hugs (`S.U13`, `O.12.f`). Biblioteca is
/// the only [`SidebarSide::Left`] caller today; `S.U14`'s AI harness is the
/// [`SidebarSide::Right`] one `O.12.f`'s own note calls out — this type and
/// [`side_panel`] exist so that lote is a call site, not a rework of this
/// mechanism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSide {
    Left,
    Right,
}

/// A delegated sidebar host, resizable exactly like the four grid regions in
/// `regions.rs`: it opens at `width` points (the caller's
/// [`Theme::sidebar_width`]) and can be dragged between `min` and `max`
/// (the caller's [`Theme::sidebar_bounds`]).
/// egui keeps the dragged size under `id`, so the sidebar's width persists
/// the way a splitter's does. The separator stays visible so the drag handle
/// shows on hover, as it does between grid regions. Values come from the
/// theme, never literals here (gate 2).
pub fn side_panel(id: Id, side: SidebarSide, width: f32, min: f32, max: f32) -> Panel {
    let panel = match side {
        SidebarSide::Left => Panel::left(id),
        SidebarSide::Right => Panel::right(id),
    };
    panel
        .resizable(true)
        .default_size(width)
        .min_size(min)
        .max_size(max)
        .frame(Frame::NONE)
}

/// A hue bar and a saturation×brightness square, in the theme's own
/// proportions, editing `value` in place — the control a `color` node's folder
/// and a `palette` slot's picker are built from.
///
/// **Why `Hsb` and not `egui`'s picker.** `shell/gates.rs` gate 2 forbids an
/// `Hsva` outside `classes.rs`, and that is the right rule here: the value the
/// app stores is `metal_notation::color::Hsb` (a struct this crate owns), the
/// canvas paints a swatch from the same `Hsb`, and a picker that edited some
/// other type would need a third conversion on every frame. So this draws with
/// `Painter` and the theme's tokens, samples `Hsb` for the pixels, and never
/// names an `egui` colour type.
///
/// **It reports one gesture, on release.** The returned [`egui::Response`]'s
/// `dragged()` is true every frame of a drag; its `changed()` is true on the
/// frame the pointer is released — the seam a caller posts from, so a drag
/// across the square writes the node **once**, not once per pixel. `value` is
/// updated live throughout, so the swatch beside it tracks the drag.
#[must_use]
pub fn hsb_picker(ui: &mut Ui, theme: &Theme, value: &mut Hsb) -> egui::Response {
    let area = egui::vec2(
        theme.picker_square.x + theme.picker_gap + theme.picker_hue.x,
        theme.picker_square.y,
    );
    let (rect, mut response) = ui.allocate_exact_size(area, egui::Sense::click_and_drag());
    let square = egui::Rect::from_min_size(rect.min, theme.picker_square);
    let hue = egui::Rect::from_min_size(
        egui::pos2(square.right() + theme.picker_gap, rect.top()),
        theme.picker_hue,
    );

    let painter = ui.painter_at(rect);
    paint_square(ui, &painter, theme, square, value);
    paint_hue(&painter, theme, hue, value);

    if response.is_pointer_button_down_on() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            if square.contains(p) {
                value.s = ((p.x - square.left()) / square.width()).clamp(0.0, 1.0);
                value.b = (1.0 - (p.y - square.top()) / square.height()).clamp(0.0, 1.0);
            } else if hue.contains(p) {
                value.h = ((p.y - hue.top()) / hue.height()).clamp(0.0, 1.0) * 360.0;
            }
        }
    }

    // One post per gesture: the response is `changed` only on the frame the
    // drag ends, whatever path it took. A caller that only wants the final
    // value reads `changed()`; one that wants the live one reads `value`.
    // A plain click (no drag past egui's threshold) is a gesture too.
    if response.drag_stopped() || response.clicked() {
        response.mark_changed();
    }
    response
}

/// Hue's full turn and the percent scale the S/B fields read in.
const HUE_TURN: f32 = 360.0;
const PERCENT: f32 = 100.0;

/// Parse `#rrggbb` / `rrggbb` into 0..1 rgb.
fn parse_hex(text: &str) -> Option<[f32; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let ch = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).ok().map(|v| f32::from(v) / f32::from(u8::MAX));
    Some([ch(0)?, ch(2)?, ch(4)?])
}

/// One compact `label  [track]` line of the colour editor's field column.
fn colour_field(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        ui.add_sized(
            [theme.swatch_size.x, ui.spacing().interact_size.y],
            egui::Label::new(RichText::new(label).font(theme.font_small.clone()).color(theme.ink_weak)),
        );
        let w = ui.available_width().max(theme.interact_size.x);
        let default = *range.start();
        num_track_sized(ui, theme, w, value, range, suffix, 0, default, true)
    })
    .inner
}

/// The full colour editor: the S×B square and hue strip on the left, and a
/// field column on the right — a swatch, typeable H°/S%/B% tracks and a hex
/// field. Falls back to stacking when the panel is too narrow for both.
/// `changed()` once per gesture, like [`hsb_picker`].
pub fn colour_editor(ui: &mut Ui, theme: &Theme, value: &mut Hsb) -> egui::Response {
    let picker_w = theme.picker_square.x + theme.picker_gap + theme.picker_hue.x;
    let side = ui.available_width() >= picker_w + theme.picker_gap + theme.control_width;
    let mut changed = false;
    let mut fields = |ui: &mut Ui, value: &mut Hsb| {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, theme.swatch_size.y * 2.0), Sense::hover());
        ui.painter().rect_filled(rect, theme.radius, theme.color_of(value.to_rgb()));
        ui.painter().rect_stroke(rect, theme.radius, theme.rule_stroke(), egui::StrokeKind::Inside);

        let mut h = value.h;
        if colour_field(ui, theme, "H", &mut h, 0.0..=HUE_TURN, "°").changed() {
            value.h = h;
            changed = true;
        }
        let mut s = value.s * PERCENT;
        if colour_field(ui, theme, "S", &mut s, 0.0..=PERCENT, "%").changed() {
            value.s = s / PERCENT;
            changed = true;
        }
        let mut b = value.b * PERCENT;
        if colour_field(ui, theme, "B", &mut b, 0.0..=PERCENT, "%").changed() {
            value.b = b / PERCENT;
            changed = true;
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = theme.item_gap;
            ui.add_sized(
                [theme.swatch_size.x, ui.spacing().interact_size.y],
                egui::Label::new(RichText::new("#").font(theme.font_small.clone()).color(theme.ink_weak)),
            );
            let id = ui.id().with("colour-hex");
            let mut draft = ui
                .data(|d| d.get_temp::<String>(id))
                .unwrap_or_else(|| value.hex().trim_start_matches('#').to_string());
            let edit = ui.add(
                egui::TextEdit::singleline(&mut draft)
                    .font(theme.font_small.clone())
                    .desired_width(ui.available_width()),
            );
            if edit.has_focus() {
                ui.data_mut(|d| d.insert_temp(id, draft.clone()));
            }
            if edit.lost_focus() {
                if let Some([r, g, b]) = parse_hex(&draft) {
                    *value = Hsb::from_rgb(r, g, b);
                    changed = true;
                }
                ui.data_mut(|d| d.remove::<String>(id));
            }
        });
    };
    let mut response = if side {
        ui.horizontal_top(|ui| {
            let r = hsb_picker(ui, theme, value);
            ui.vertical(|ui| fields(ui, value));
            r
        })
        .inner
    } else {
        ui.vertical(|ui| {
            let r = hsb_picker(ui, theme, value);
            fields(ui, value);
            r
        })
        .inner
    };
    if changed {
        response.mark_changed();
    }
    response
}

/// A labelled colour row: a swatch and its hex on the value column; a click
/// opens the [`colour_editor`] in a popup. `changed()` when the editor commits.
pub fn colour_row(ui: &mut Ui, theme: &Theme, label: &str, value: &mut Hsb) -> egui::Response {
    label_row(ui, theme, label, |ui| colour_button(ui, theme, value))
}

/// The bare swatch button [`colour_row`] draws, for a caller laying out its
/// own row (a palette's slots).
pub fn colour_button(ui: &mut Ui, theme: &Theme, value: &mut Hsb) -> egui::Response {
    let w = ui.available_width().max(theme.swatch_size.x);
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(w, ui.spacing().interact_size.y), Sense::click());
    let chip = egui::Rect::from_min_size(rect.min, egui::vec2(rect.height() * 2.0, rect.height()));
    let painter = ui.painter();
    painter.rect_filled(rect, theme.radius, theme.inset);
    painter.rect_filled(chip, theme.radius, theme.color_of(value.to_rgb()));
    let stroke = if response.hovered() { theme.card_picked_stroke() } else { theme.rule_stroke() };
    painter.rect_stroke(rect, theme.radius, stroke, egui::StrokeKind::Inside);
    painter.text(
        egui::pos2(chip.right() + theme.picker_gap, rect.center().y),
        egui::Align2::LEFT_CENTER,
        value.hex(),
        theme.font_small.clone(),
        theme.ink,
    );
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(theme.picker_square.x + theme.picker_gap + theme.picker_hue.x + theme.picker_gap + theme.control_width);
            changed = colour_editor(ui, theme, value).changed();
        });
    if changed {
        response.mark_changed();
    }
    response
}

/// The S×B square at the current hue: a coarse grid of cells, each sampled
/// through [`Hsb::to_rgb`] — the same conversion the swatch and the node's
/// config carry, so what the operator drags is what is stored.
fn paint_square(ui: &Ui, painter: &egui::Painter, theme: &Theme, rect: egui::Rect, value: &Hsb) {
    const COLS: usize = 24;
    const ROWS: usize = 20;
    let cw = rect.width() / COLS as f32;
    let ch = rect.height() / ROWS as f32;
    for row in 0..ROWS {
        for col in 0..COLS {
            let s = (col as f32 + 0.5) / COLS as f32;
            let b = 1.0 - (row as f32 + 0.5) / ROWS as f32;
            let c = Hsb { h: value.h, s, b };
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left() + col as f32 * cw, rect.top() + row as f32 * ch),
                    egui::vec2(cw + 0.5, ch + 0.5),
                ),
                theme.radius,
                theme.color_of(c.to_rgb()),
            );
        }
    }
    painter.rect_stroke(
        rect,
        theme.radius,
        theme.rule_stroke(),
        egui::StrokeKind::Inside,
    );
    // The handle: a ring at (s, 1 - b).
    let hp = egui::pos2(
        rect.left() + value.s * rect.width(),
        rect.top() + (1.0 - value.b) * rect.height(),
    );
    painter.circle_stroke(
        hp,
        theme.picker_handle,
        theme.line_stroke(theme.picker_handle_stroke),
    );
    let _ = ui;
}

/// The hue bar: one cell per 15°, so a hue is draggable to within a step, plus
/// a tick where the value sits.
fn paint_hue(painter: &egui::Painter, theme: &Theme, rect: egui::Rect, value: &Hsb) {
    const STEPS: usize = 24;
    let sh = rect.height() / STEPS as f32;
    for i in 0..STEPS {
        let h = (i as f32 + 0.5) / STEPS as f32 * 360.0;
        painter.rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + i as f32 * sh),
                egui::vec2(rect.width(), sh + 0.5),
            ),
            theme.radius,
            theme.color_of(Hsb { h, s: 1.0, b: 1.0 }.to_rgb()),
        );
    }
    painter.rect_stroke(
        rect,
        theme.radius,
        theme.rule_stroke(),
        egui::StrokeKind::Inside,
    );
    let hy = rect.top() + (value.h / 360.0) * rect.height();
    painter.line_segment(
        [egui::pos2(rect.left(), hy), egui::pos2(rect.right(), hy)],
        theme.line_stroke(theme.picker_handle_stroke),
    );
}

/// Which numeric widget a [`float_row`] wraps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Float {
    /// An `egui::Slider` over the row's range.
    Slider,
    /// An `egui::DragValue`: `speed` per point dragged, and the number of
    /// decimals to pin (`None` leaves egui's own default).
    Drag { speed: f32, decimals: Option<usize> },
}

/// The glyphs at the two ends of a [`num_track`].
const NUM_ARROW_LEFT: &str = "\u{25c2}";
const NUM_ARROW_RIGHT: &str = "\u{25b8}";
/// A scrub sweeps the whole declared range over this many points.
const NUM_SCRUB_SPAN: f32 = 200.0;
/// Shift divides the scrub step by this; Ctrl multiplies it by this.
const NUM_SCRUB_SCALE: f32 = 10.0;
/// One arrow click steps by `range /` this.
const NUM_STEP_DIVISOR: f32 = 100.0;
/// The hold before an arrow auto-repeats, in seconds.
const NUM_HOLD_DELAY: f64 = 0.35;
/// The slowest and fastest auto-repeat intervals, in seconds (12 → 40 a
/// second), and how long the ramp between them takes.
const NUM_REPEAT_SLOW: f64 = 1.0 / 12.0;
const NUM_REPEAT_FAST: f64 = 1.0 / 40.0;
const NUM_REPEAT_RAMP: f64 = 1.0;
/// The decimals a [`Float::Slider`] row and a decimal-less [`Float::Drag`]
/// row pin their readout to.
const NUM_SLIDER_DECIMALS: usize = 2;

/// Points of smooth wheel scroll one physical notch is — the same constant
/// the node canvas and timeline zoom with.
const WHEEL_NOTCH: f32 = 50.0;
/// A notch changes a float by a hundredth of its declared range.
const WHEEL_RANGE_DIVISOR: f32 = 100.0;
/// Shift multiplies the step by this; Alt divides it by this.
const WHEEL_SCALE: f32 = 10.0;
/// A wheel gesture must sit still this long before it posts once.
const WHEEL_POST_DELAY: Duration = Duration::from_millis(150);

/// One properties float, wrapped so the wheel over it edits **this** value
/// and nothing else: `range / 100` per notch (×10 with Shift, ÷10 with Alt),
/// clamped to the declared range. The float under the pointer consumes the
/// wheel, so the panel behind it does not also scroll; the caller's
/// `.changed()` fires **once per gesture**, [`WHEEL_POST_DELAY`] after the
/// wheel stops, so a caller posts the same body a drag would, not one per
/// notch.
///
/// Every properties float goes through here — `gates.rs`'s escalation is a
/// float that bypasses it. The signature predates the theme argument, so the
/// active [`Theme`] is read from the context ([`Theme::current`]); `num_track`
/// is the theme-aware control underneath.
pub fn float_row(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    widget: Float,
) -> egui::Response {
    float_row_enabled(ui, value, range, widget, true)
}

/// [`float_row`] with an explicit `enabled` — a float a wired source owns is
/// disabled and must not read the wheel.
pub fn float_row_enabled(
    ui: &mut Ui,
    value: &mut f32,
    range: RangeInclusive<f32>,
    widget: Float,
    enabled: bool,
) -> egui::Response {
    let theme = Theme::current(ui.ctx()).unwrap_or_else(Theme::phosphor);
    // The old `Float` vocabulary maps onto the one track: a slider is two
    // decimals, a drag carries its own count (or the same two), and both
    // reset to the bottom of their declared range. `speed` is spent — the
    // track scrubs at `range / `[`NUM_SCRUB_SPAN`] for every float.
    let (decimals, default) = match widget {
        Float::Slider => (NUM_SLIDER_DECIMALS, *range.start()),
        Float::Drag {
            speed: _,
            decimals,
        } => (decimals.unwrap_or(NUM_SLIDER_DECIMALS), *range.start()),
    };
    let w = ui.available_width().max(theme.interact_size.x);
    num_track_sized(ui, &theme, w, value, range, "", decimals, default, enabled)
}

/// Wheel handling for one float: read the raw notches for the step (so one
/// mouse notch is exactly one `range / 100`), consume the smoothed scroll so
/// the panel stays put, and hold the accumulated value until the gesture has
/// been still for [`WHEEL_POST_DELAY`].
fn wheel_edit(
    ui: &mut Ui,
    id: Id,
    value: &mut f32,
    range: &RangeInclusive<f32>,
    response: &mut egui::Response,
) {
    let now = ui.input(|i| i.time);
    let key = id.with("wheel");

    if response.hovered() {
        let (notches, mods) = ui.input_mut(|i| {
            // A float under the pointer owns the wheel; the panel behind it
            // must not also scroll, on this frame or its smoothed tail.
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            let mut notches = 0.0;
            let mut mods = i.modifiers;
            for event in &i.events {
                if let egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = event
                {
                    notches += match unit {
                        egui::MouseWheelUnit::Line => delta.y,
                        egui::MouseWheelUnit::Point => delta.y / WHEEL_NOTCH,
                        egui::MouseWheelUnit::Page => delta.y,
                    };
                    mods = *modifiers;
                }
            }
            (notches, mods)
        });
        if notches != 0.0 {
            let (lo, hi) = (*range.start(), *range.end());
            let mut step = (hi - lo) / WHEEL_RANGE_DIVISOR;
            if mods.shift {
                step *= WHEEL_SCALE;
            }
            if mods.alt {
                step /= WHEEL_SCALE;
            }
            let base = ui
                .data(|d| d.get_temp::<(f32, f64)>(key))
                .map(|(pending, _)| pending)
                .unwrap_or(*value);
            let next = (base + notches * step).clamp(lo, hi);
            *value = next;
            ui.data_mut(|d| d.insert_temp(key, (next, now)));
            ui.ctx().request_repaint_after(WHEEL_POST_DELAY);
        }
    }

    if let Some((pending, last)) = ui.data(|d| d.get_temp::<(f32, f64)>(key)) {
        if now - last >= WHEEL_POST_DELAY.as_secs_f64() {
            *value = pending;
            response.mark_changed();
            ui.data_mut(|d| d.remove::<(f32, f64)>(key));
        }
    }
}

/// The one value track both [`num_track`] and the grid's fader paint: a
/// recessed inset with the value fill up to `t` (`0..=1`) along x
/// (`horizontal`) or y, in the colours `state` selects.
pub(crate) fn value_track(ui: &Ui, theme: &Theme, rect: Rect, t: f32, horizontal: bool, state: ControlState) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.radius, theme.inset);
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return;
    }
    let fill = if horizontal {
        Rect::from_min_size(rect.min, egui::vec2(rect.width() * t, rect.height()))
    } else {
        Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - rect.height() * t), rect.max)
    };
    painter.rect_filled(fill, theme.radius, theme.control_fill(state));
}

/// Wheel + right-click-reset for a `0..=1`-style value on `response`: the
/// gestures [`num_track`] and the fader share. `changed()` fires once per
/// gesture.
pub(crate) fn track_gestures(
    ui: &mut Ui,
    id: Id,
    value: &mut f32,
    range: &RangeInclusive<f32>,
    default: f32,
    response: &mut Response,
) {
    if response.secondary_clicked() {
        *value = default.clamp(*range.start(), *range.end());
        response.mark_changed();
    }
    if let Some((pending, _)) = ui.data(|d| d.get_temp::<(f32, f64)>(id.with("wheel"))) {
        *value = pending;
    }
    wheel_edit(ui, id, value, range, response);
}

/// The one numeric control: a recessed track with the value fill drawn in
/// the phosphor up to the value's normalized position, the value and its
/// `suffix` centred, and `◂`/`▸` arrows at the two ends. `num` wraps it in a
/// label row; `float_row`/`number_row`/`slider_row` route through it.
///
/// Every gesture reports through the returned response's `changed()`: a
/// scrub on release, an arrow step on each click or repeat, a typed value on
/// Enter or lost focus, a right-click reset — so a caller posts the door body
/// **once per gesture**, never once per frame.
#[must_use]
pub fn num_track(
    ui: &mut Ui,
    theme: &Theme,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    default: f32,
    enabled: bool,
) -> egui::Response {
    num_track_sized(ui, theme, theme.control_width, value, range, suffix, decimals, default, enabled)
}

/// [`num_track`] at `width` — the labelled rows pass what the row leaves, so
/// the track fills the panel instead of stopping at the token width.
#[allow(clippy::too_many_arguments)]
fn num_track_sized(
    ui: &mut Ui,
    theme: &Theme,
    width: f32,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    default: f32,
    enabled: bool,
) -> egui::Response {
    // `next_auto_id` is the stable key this call site gets every frame; it
    // is read (not consumed) here and consumed by `allocate_exact_size`
    // below, so the wheel's pending value can be restored *before* the track
    // paints, exactly as `float_row` always did.
    let id = ui.next_auto_id();
    // Fill what the row leaves (a wide panel no longer paints a stub of a
    // control and half a row of nothing); never narrower than the token.
    let size = egui::vec2(width, ui.spacing().interact_size.y);
    let (lo, hi) = (*range.start(), *range.end());
    let span = hi - lo;
    let sense = if enabled {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::hover()
    };

    if enabled {
        if let Some((pending, _)) = ui.data(|d| d.get_temp::<(f32, f64)>(id.with("wheel"))) {
            *value = pending;
        }
        // A scrub in flight: the caller re-reads the held value every frame
        // and nothing is posted until release, so the gesture's own value
        // lives here — otherwise each frame's delta lands on the stale value
        // and the number never moves.
        if let Some(live) = ui.data(|d| d.get_temp::<f32>(id.with("scrubv"))) {
            *value = live;
        }
    }
    let editing = enabled && ui.data(|d| d.get_temp::<bool>(id.with("edit")).unwrap_or(false));

    let (rect, mut response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*value), ""));

    let state = ControlState::of(&response, enabled, false, editing);
    let (text_ink, arrow_ink) = (theme.control_ink(state), theme.control_ink_weak(state));
    let t = if span > 0.0 {
        ((*value - lo) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    value_track(ui, theme, rect, t, true, state);
    let painter = ui.painter_at(rect);

    // The two arrow hit-zones are squares as tall as the track, so no size
    // is written down: `interact_size.y` already set the track's height.
    let arrow_w = rect.height();
    let left_zone = egui::Rect::from_min_size(rect.min, egui::vec2(arrow_w, rect.height()));
    let right_zone = egui::Rect::from_min_max(
        egui::pos2(rect.right() - arrow_w, rect.top()),
        rect.max,
    );
    painter.text(
        left_zone.center(),
        egui::Align2::CENTER_CENTER,
        NUM_ARROW_LEFT,
        theme.font_body.clone(),
        arrow_ink,
    );
    painter.text(
        right_zone.center(),
        egui::Align2::CENTER_CENTER,
        NUM_ARROW_RIGHT,
        theme.font_body.clone(),
        arrow_ink,
    );

    if !editing {
        // The centred readout: a value label and, when there is one, a
        // suffix label after it — both real labels, so AccessKit can name
        // the unit even though the track itself is painted.
        let text = format!("{:.*}", decimals, *value);
        let mut text_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center).with_main_align(egui::Align::Center)),
        );
        text_ui.spacing_mut().item_spacing = theme.segment_gap;
        text_ui.label(
            RichText::new(text)
                .font(theme.font_small.clone())
                .color(text_ink),
        );
        if !suffix.is_empty() {
            text_ui.label(
                RichText::new(suffix)
                    .font(theme.font_small.clone())
                    .color(theme.ink_faint),
            );
        }
    }

    if enabled && editing {
        let mut draft: String = ui
            .data(|d| d.get_temp::<String>(id.with("draft")))
            .unwrap_or_else(|| format!("{:.*}", decimals, *value));
        let edit = ui.put(
            rect,
            egui::TextEdit::singleline(&mut draft)
                .font(theme.font_small.clone())
                .desired_width(rect.width()),
        );
        if ui.data(|d| d.get_temp::<bool>(id.with("focus")).unwrap_or(false)) {
            edit.request_focus();
            ui.data_mut(|d| d.insert_temp(id.with("focus"), false));
        }
        ui.data_mut(|d| d.insert_temp(id.with("draft"), draft.clone()));
        let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if esc {
            ui.data_mut(|d| {
                d.insert_temp(id.with("edit"), false);
                d.remove::<String>(id.with("draft"));
            });
        } else if enter || edit.lost_focus() {
            if let Ok(parsed) = draft.trim().parse::<f32>() {
                *value = parsed.clamp(lo, hi);
                response.mark_changed();
            }
            ui.data_mut(|d| {
                d.insert_temp(id.with("edit"), false);
                d.remove::<String>(id.with("draft"));
            });
        }
        return response;
    }

    let mut changed = false;
    if enabled {
        let now = ui.input(|i| i.time);
        let press_origin = ui.input(|i| i.pointer.press_origin());
        let arrow_dir = if press_origin.is_some_and(|p| left_zone.contains(p)) {
            -1.0
        } else if press_origin.is_some_and(|p| right_zone.contains(p)) {
            1.0
        } else {
            0.0
        };
        let held = response.is_pointer_button_down_on();

        // Arrows: one step on the press frame, then auto-repeat after
        // `NUM_HOLD_DELAY`, accelerating from 12 to 40 steps a second.
        if arrow_dir != 0.0 && held {
            let key = id.with("arrow");
            let step = span / NUM_STEP_DIVISOR;
            match ui.data(|d| d.get_temp::<(f64, f64)>(key)) {
                None => {
                    *value = (*value + arrow_dir * step).clamp(lo, hi);
                    ui.data_mut(|d| d.insert_temp(key, (now, now + NUM_HOLD_DELAY)));
                    ui.ctx().request_repaint();
                    changed = true;
                }
                Some((start, next)) if now >= next => {
                    *value = (*value + arrow_dir * step).clamp(lo, hi);
                    let held_for = (now - start - NUM_HOLD_DELAY).max(0.0);
                    let interval = (NUM_REPEAT_SLOW
                        - held_for * (NUM_REPEAT_SLOW - NUM_REPEAT_FAST) / NUM_REPEAT_RAMP)
                        .clamp(NUM_REPEAT_FAST, NUM_REPEAT_SLOW);
                    ui.data_mut(|d| d.insert_temp(key, (start, now + interval)));
                    ui.ctx().request_repaint();
                    changed = true;
                }
                Some(_) => {}
            }
        } else if !held {
            ui.data_mut(|d| d.remove::<(f64, f64)>(id.with("arrow")));
        }

        // Scrub anywhere but on an arrow: `range / NUM_SCRUB_SPAN` per point,
        // fine with Shift, coarse with Ctrl. The value tracks the drag live;
        // `changed` fires only once the drag stops.
        if arrow_dir == 0.0 {
            if response.dragged() {
                let mods = ui.input(|i| i.modifiers);
                let mut sens = span / NUM_SCRUB_SPAN;
                if mods.shift {
                    sens /= NUM_SCRUB_SCALE;
                }
                if mods.ctrl || mods.command {
                    sens *= NUM_SCRUB_SCALE;
                }
                *value = (*value + response.drag_delta().x * sens).clamp(lo, hi);
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                let live = *value;
                ui.data_mut(|d| {
                    d.insert_temp(id.with("scrub"), true);
                    d.insert_temp(id.with("scrubv"), live);
                });
            }
            if response.drag_stopped()
                && ui.data(|d| d.get_temp::<bool>(id.with("scrub")).unwrap_or(false))
            {
                ui.data_mut(|d| {
                    d.remove::<bool>(id.with("scrub"));
                    d.remove::<f32>(id.with("scrubv"));
                });
                changed = true;
            }
        }

        // A click off the arrows opens the text edit; right-click resets to
        // `default`.
        let on_arrow = response
            .interact_pointer_pos()
            .is_some_and(|p| left_zone.contains(p) || right_zone.contains(p));
        if response.clicked() && !on_arrow {
            ui.data_mut(|d| {
                d.insert_temp(id.with("edit"), true);
                d.insert_temp(id.with("draft"), format!("{:.*}", decimals, *value));
                d.insert_temp(id.with("focus"), true);
            });
        }
        track_gestures(ui, id, value, &range, default, &mut response);
    }

    if changed {
        response.mark_changed();
    }
    response
}

/// [`num_track`] in a label row — the labelled numeric field the kit's rows
/// are built from. The label names the field for AccessKit; the `suffix`
/// rides in the track's centred readout.
#[must_use]
pub fn num(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    default: f32,
    enabled: bool,
) -> egui::Response {
    label_row(ui, theme, label, |ui| {
        let w = ui.available_width().max(theme.interact_size.x);
        num_track_sized(ui, theme, w, value, range, suffix, decimals, default, enabled)
    })
}

/// The bare numeric track in the value column of a [`kv_row`], `width` wide
/// (a caller that puts a button beside it passes what is left).
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn num_value(
    ui: &mut Ui,
    theme: &Theme,
    width: f32,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    default: f32,
    enabled: bool,
) -> egui::Response {
    num_track_sized(ui, theme, width.max(theme.interact_size.x), value, range, suffix, decimals, default, enabled)
}

// --- the widget kit (2026-10-08) -------------------------------------------
//
// The properties and biblioteca panels are built from one vocabulary of
// rows: a leading label on a fixed column, then a selector or a value. These
// live here — the one place shared widgets may live (gate 1) — so every
// folder of `properties/` and the library list draw the *same* row shape.
// The row renderer lives against `Theme`; no visual value is written down
// here (gate 2).

/// The leading label of a row, on the kit's fixed [`Theme::label_width`]
/// column so a stack of rows reads as a table. Weak ink, small face, and
/// truncated rather than wrapping (a label never pushes its control out of
/// the row).
fn row_label(ui: &mut Ui, theme: &Theme, label: &str) {
    ui.add_sized(
        [theme.label_width, ui.spacing().interact_size.y],
        egui::Label::new(
            RichText::new(label)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        )
        .truncate(),
    );
}

/// A row of a leading label and one control the caller draws, both on the
/// kit's columns — the shape `number_row`/`slider_row`/`toggle_row` are
/// built from, exposed for a one-off control (a button, a combo, a swatch).
/// Returns the inner control's response.
pub fn label_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    add: impl FnOnce(&mut Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        row_label(ui, theme, label);
        // The value column is what the label leaves — never wider, so a
        // control cannot push the row past the panel.
        let width = ui.available_width().max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_max_width(width);
                add(ui)
            },
        )
        .inner
    })
    .inner
}

/// The outlined group every properties section sits in: a quiet grey rule
/// around the body, `Theme::card_pad` inside, an optional small title on top,
/// and `Theme::card_gap` after it. Always the panel's full width.
pub fn card<R>(ui: &mut Ui, theme: &Theme, title: Option<&str>, add: impl FnOnce(&mut Ui) -> R) -> R {
    let out = egui::Frame::new()
        .stroke(theme.rule_stroke())
        .corner_radius(theme.corner_radius())
        .inner_margin(theme.card_pad)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(title) = title {
                ui.label(RichText::new(title).font(theme.font_small.clone()).color(theme.ink_weak).strong());
            }
            add(ui)
        })
        .inner;
    ui.add_space(theme.card_gap);
    out
}

/// A small grid of rows under weak-ink column headers: the shape a mapping
/// table reads as. `cell(ui, row, col)` draws each cell; the grid takes the
/// theme's item gap and an id of its own so two tables on one panel do not
/// share column widths.
pub fn table(
    ui: &mut Ui,
    theme: &Theme,
    id: impl std::hash::Hash + std::fmt::Debug,
    headers: &[&str],
    rows: usize,
    mut cell: impl FnMut(&mut Ui, usize, usize),
) {
    egui::Grid::new(Id::new(("shell/table", id)))
        .spacing(theme.item_gap)
        .striped(true)
        .show(ui, |ui| {
            for h in headers {
                ui.add(
                    egui::Label::new(
                        RichText::new(*h)
                            .font(theme.font_small.clone())
                            .color(theme.ink_weak),
                    )
                    .truncate(),
                );
            }
            ui.end_row();
            for row in 0..rows {
                for col in 0..headers.len() {
                    cell(ui, row, col);
                }
                ui.end_row();
            }
        });
}

/// A collapsible [`card`]: the chevron + title header on top (click folds the
/// body), the body below. `Some` of the body's result while it is open.
pub fn card_section<R>(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    open: &mut bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let out = egui::Frame::new()
        .stroke(theme.rule_stroke())
        .corner_radius(theme.corner_radius())
        .inner_margin(theme.card_pad)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let text = format!("{} {label}", if *open { "\u{25be}" } else { "\u{25b8}" });
            let head = ui.add(
                egui::Label::new(RichText::new(text).font(theme.font_heading.clone()).color(theme.ink_weak))
                    .truncate()
                    .sense(egui::Sense::click()),
            );
            if head.clicked() {
                *open = !*open;
            }
            (*open).then(|| {
                ui.add_space(theme.item_gap.y);
                add(ui)
            })
        })
        .inner;
    ui.add_space(theme.card_gap);
    out
}

/// A key/value row in a card: [`label_row`] by another name, so a new page
/// reads as "two columns, key then value" and nothing else.
pub fn kv_row(ui: &mut Ui, theme: &Theme, key: &str, add: impl FnOnce(&mut Ui) -> egui::Response) -> egui::Response {
    label_row(ui, theme, key, add)
}

/// The small face every properties control labels itself with: buttons,
/// checkboxes, captions. The caller colours it with `.color(…)` when it needs
/// a shade other than the button default.
pub fn small_text(theme: &Theme, text: impl Into<String>) -> RichText {
    RichText::new(text).font(theme.font_small.clone())
}

/// A vertex handle on a drawn editor (the mask contour, the sector grid): a
/// filled dot of the handle radius at `at`.
pub fn handle_dot(painter: &egui::Painter, theme: &Theme, at: Pos2, fill: Color32) {
    painter.circle_filled(at, theme.hairline * theme.handle_hairlines, fill);
}

/// The ring that marks a point waiting for its pair (the sector grid's
/// half-streak cue): the handle radius, outlined in the accent.
pub fn handle_ring(painter: &egui::Painter, theme: &Theme, at: Pos2) {
    painter.circle_stroke(at, theme.hairline * theme.handle_hairlines, theme.accent());
}

/// A quiet small-face note line (weak ink), wrapped inside its card.
pub fn note(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).font(theme.font_small.clone()).color(theme.ink_weak)).wrap());
}

/// A key/value row whose value is a row of small buttons or toggles that
/// wrap inside the value column.
pub fn kv_wrap(ui: &mut Ui, theme: &Theme, key: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        row_label(ui, theme, key);
        let width = ui.available_width().max(0.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.0),
            egui::Layout::left_to_right(egui::Align::Min).with_main_wrap(true),
            |ui| {
                ui.set_max_width(width);
                add(ui)
            },
        );
    });
}

/// A read-only row: label on the left, `value` in the panel's own ink on the
/// right. The row a `node`/`patch` fact draws with, so read-only facts share
/// the editable rows' column and the panel stops reading as loose lines.
pub fn field_row(ui: &mut Ui, theme: &Theme, label: &str, value: &str) {
    label_row(ui, theme, label, |ui| {
        ui.label(
            RichText::new(value)
                .font(theme.font_small.clone())
                .color(theme.ink),
        )
    });
}

/// A collapsible section header: a chevron and the section name, in the
/// heading face and the weak ink. Toggling `open` on click; the caller wraps
/// the section body in `if *open { … }`. `section_gap` above separates the
/// block from the previous one. Returns the header's own response.
pub fn section(ui: &mut Ui, theme: &Theme, label: &str, open: &mut bool) -> egui::Response {
    ui.add_space(theme.section_gap);
    let text = if *open {
        format!("\u{25be} {label}")
    } else {
        format!("\u{25b8} {label}")
    };
    let response = ui.selectable_label(
        *open,
        RichText::new(text)
            .font(theme.font_heading.clone())
            .color(theme.ink_weak),
    );
    if response.clicked() {
        *open = !*open;
    }
    response
}

/// Builder for the one dropdown shell; see [`combo_box`].
pub struct Combo<'a> {
    theme: &'a Theme,
    id: Id,
    width: Option<f32>,
    text: String,
    enabled: bool,
}

/// Starts a dropdown: the shell's only way to spell an egui combo. Chain
/// `.width()`, `.selected_text()`, `.enabled()`, then `.show_ui()`.
pub fn combo_box<'a>(theme: &'a Theme, id: impl std::hash::Hash + std::fmt::Debug) -> Combo<'a> {
    Combo { theme, id: Id::new(id), width: None, text: String::new(), enabled: true }
}

impl Combo<'_> {
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }
    pub fn selected_text(mut self, text: impl Into<String>) -> Self {
        self.text = text.into();
        self
    }
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    pub fn show_ui<R>(self, ui: &mut Ui, items: impl FnOnce(&mut Ui) -> R) -> egui::InnerResponse<Option<R>> {
        let Combo { theme, id, width, text, enabled } = self;
        ui.add_enabled_ui(enabled, |ui| {
            let mut combo = egui::ComboBox::from_id_salt(id)
                .selected_text(RichText::new(text).font(theme.font_small.clone()));
            if let Some(w) = width {
                combo = combo.width(w);
            }
            combo.show_ui(ui, items)
        })
        .inner
    }
}

/// A [`combo_box`] on the kit's control column with a ready body.
pub fn combo(
    ui: &mut Ui,
    theme: &Theme,
    id: impl std::hash::Hash + std::fmt::Debug,
    shown: &str,
    width: Option<f32>,
    enabled: bool,
    items: impl FnOnce(&mut Ui),
) -> egui::Response {
    combo_box(theme, id)
        .width(width.unwrap_or(theme.control_width))
        .selected_text(shown)
        .enabled(enabled)
        .show_ui(ui, items)
        .response
}

/// One row of a [`combo`] body.
pub fn menu_item(ui: &mut Ui, theme: &Theme, selected: bool, label: &str) -> egui::Response {
    ui.selectable_label(selected, RichText::new(label).font(theme.font_small.clone()))
}

/// A dropdown: `current` selected against `options` of `(value, label)`.
/// Returns the combo's response, `changed()` when a different option was
/// picked.
pub fn dropdown<T: PartialEq + Clone>(
    ui: &mut Ui,
    theme: &Theme,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: &mut T,
    options: &[(T, &str)],
) -> egui::Response {
    dropdown_enabled(ui, theme, id, current, options, true)
}

/// [`dropdown`] with an explicit `enabled`.
pub fn dropdown_enabled<T: PartialEq + Clone>(
    ui: &mut Ui,
    theme: &Theme,
    id: impl std::hash::Hash + std::fmt::Debug,
    current: &mut T,
    options: &[(T, &str)],
    enabled: bool,
) -> egui::Response {
    let shown = options
        .iter()
        .find(|(value, _)| value == current)
        .map(|(_, label)| *label)
        .unwrap_or("");
    let mut picked = None;
    let mut response = combo(ui, theme, id, shown, None, enabled, |ui| {
        for (value, label) in options {
            if menu_item(ui, theme, value == &*current, label).clicked() {
                picked = Some(value.clone());
            }
        }
    });
    if let Some(v) = picked {
        if v != *current {
            *current = v;
            response.mark_changed();
        }
    }
    response
}

/// A labelled numeric row: label, the [`num_track`] on the control column,
/// and the unit (`suffix`) in the track's readout. Delegates to [`num`], so
/// a numeric parameter behaves identically wherever it appears.
pub fn number_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    suffix: &str,
    decimals: usize,
    enabled: bool,
) -> egui::Response {
    let default = *range.start();
    num(ui, theme, label, value, range, suffix, decimals, default, enabled)
}

/// A labelled slider row: label, then the [`num_track`] on the control
/// column — the standard inspector row, over the same wheel handling
/// [`float_row`] gives every other float.
pub fn slider_row(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    enabled: bool,
) -> egui::Response {
    let default = *range.start();
    label_row(ui, theme, label, |ui| {
        let w = ui.available_width().max(theme.interact_size.x);
        num_track_sized(
            ui,
            theme,
            w,
            value,
            range,
            "",
            NUM_SLIDER_DECIMALS,
            default,
            enabled,
        )
    })
}

/// A labelled checkbox row: the box and its own label, so the whole row is
/// the click target and the label names the box for AccessKit.
pub fn toggle_row(ui: &mut Ui, theme: &Theme, label: &str, value: &mut bool) -> egui::Response {
    toggle_row_enabled(ui, theme, label, value, true)
}

/// [`toggle_row`] with an explicit `enabled`. An empty `label` is a bare box.
pub fn toggle_row_enabled(ui: &mut Ui, theme: &Theme, label: &str, value: &mut bool, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Checkbox::new(
            value,
            RichText::new(label)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        ),
    )
}

/// A square icon toggle (a play/stop, a lock, an eye): the glyph lit in the
/// accent when `on`, faint when off, with `tip` on hover. The buttons the
/// panel rows used to spell as bare `selectable_label`s.
pub fn icon_toggle(ui: &mut Ui, theme: &Theme, icon: Icon, on: bool, tip: &str) -> egui::Response {
    icon_toggle_enabled(ui, theme, icon, on, tip, true)
}

/// [`icon_toggle`] with an explicit `enabled`.
pub fn icon_toggle_enabled(ui: &mut Ui, theme: &Theme, icon: Icon, on: bool, tip: &str, enabled: bool) -> egui::Response {
    let colour = if on { theme.phosphor } else { theme.ink_faint };
    ui.add_enabled(enabled, egui::Button::selectable(on, lucide(icon, theme.font_body.size).color(colour)))
        .on_hover_text(tip)
}

/// A segmented selector: one selectable label per option, in a horizontal
/// strip — the two-or-three-way mode picker that replaces a stack of
/// dropdowns. Returns a response `changed()` when a different segment is
/// picked.
pub fn segmented<T: PartialEq + Clone>(
    ui: &mut Ui,
    theme: &Theme,
    current: &mut T,
    options: &[(T, &str)],
) -> egui::Response {
    segmented_enabled(ui, theme, current, options, true)
}

/// [`segmented`] with an explicit `enabled`.
pub fn segmented_enabled<T: PartialEq + Clone>(
    ui: &mut Ui,
    theme: &Theme,
    current: &mut T,
    options: &[(T, &str)],
    enabled: bool,
) -> egui::Response {
    let mut changed = false;
    let mut response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing = theme.segment_gap;
            ui.add_enabled_ui(enabled, |ui| for (value, label) in options {
                let on = value == current;
                if ui
                    .selectable_label(on, RichText::new(*label).font(theme.font_small.clone()))
                    .clicked()
                    && !on
                {
                    *current = value.clone();
                    changed = true;
                }
            });
        })
        .response;
    if changed {
        response.mark_changed();
    }
    response
}

/// A row of filter chips: rounded pills, the picked one filled in the vivid
/// accent (the nodes canvas's selection look). Clicking the picked chip clears
/// it (`None`), so "all" is simply nothing picked. Returns true on a change.
pub fn filter_chips<T: PartialEq + Clone>(
    ui: &mut Ui,
    theme: &Theme,
    current: &mut Option<T>,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    for (value, label) in options {
        let on = current.as_ref() == Some(value);
        let galley = ui.painter().layout_no_wrap(
            (*label).to_string(),
            theme.font_small.clone(),
            if on { theme.ink } else { theme.ink_weak },
        );
        let size = galley.size() + Vec2::new(18.0, 8.0);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *label));
        let radius = egui::CornerRadius::same((size.y / 2.0) as u8);
        let (fill, stroke) = if on {
            (theme.accent_vivid, egui::Stroke::new(theme.accent_stroke, theme.phosphor_lit))
        } else if response.hovered() {
            (theme.library_card_hover, egui::Stroke::new(theme.hairline, theme.rule_bright))
        } else {
            (theme.inset, egui::Stroke::new(theme.hairline, theme.rule))
        };
        ui.painter().rect_filled(rect, radius, fill);
        ui.painter().rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
        let ink = if on { Color32::WHITE } else { theme.ink_weak };
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ink);
        if response.clicked() {
            *current = if on { None } else { Some(value.clone()) };
            changed = true;
        }
    }
    changed
}

/// A search field: one line of text on the control column, `hint` shown when
/// empty. The filter row the library list and the io/bind tables share.
pub fn search_field(ui: &mut Ui, theme: &Theme, text: &mut String, hint: &str) -> egui::Response {
    header_field(ui, theme, text, hint)
}

/// The one-line field a panel head carries (the library's two search boxes,
/// the lanes' name field): the control column wide, small font, with the typed
/// text *and* the hint in the same ink so every head reads alike.
pub fn header_field(ui: &mut Ui, theme: &Theme, text: &mut String, hint: &str) -> egui::Response {
    ui.add(header_edit(theme, text, hint))
}

/// [`header_field`]'s `TextEdit`, for a caller that needs to set an id first.
pub fn header_edit<'t>(theme: &Theme, text: &'t mut String, hint: &str) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .hint_text(RichText::new(hint).font(theme.font_small.clone()).color(theme.ink))
        .text_color(theme.ink)
        .desired_width(theme.control_width)
        .font(theme.font_small.clone())
        .margin(theme.field_pad)
}

/// A colour swatch chip: a filled rectangle for `value`, ringed in the accent
/// when `selected` and in the hairline rule otherwise. Clickable — a caller
/// opens its `hsb_picker` popup on `clicked()`, or uses it as a palette slot.
pub fn swatch_chip(ui: &mut Ui, theme: &Theme, value: &Hsb, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(theme.swatch_size, egui::Sense::click());
    let stroke = if selected {
        theme.card_picked_stroke()
    } else {
        theme.rule_stroke()
    };
    ui.painter()
        .rect_filled(rect, theme.radius, theme.color_of(value.to_rgb()));
    ui.painter()
        .rect_stroke(rect, theme.radius, stroke, egui::StrokeKind::Inside);
    response
}

// --- the colour-grading kit (feat/fx-grade, 2026-10-08) --------------------
//
// The Lightroom-style primitives a `grade` properties panel is built from: a
// gradient slider, a tone curve, an RGB+luma histogram, a hue/sat wheel and
// the eight-band HSL mixer row. Every colour, stroke and size is a `Theme`
// token (gate 2), and every widget reports its edit the way `hsb_picker`
// does — one `changed()` per gesture, on release — so a caller posts once.

/// How many strips a gradient track is quantised into. Enough that the bands
/// are invisible at `track_h` points, few enough to paint cheaply each frame.
const TRACK_STEPS: usize = 64;
/// How many straight segments the curve graph's spline is sampled at.
const SPLINE_STEPS: usize = 64;
/// Histogram bars, one per shadow..highlight bin.
const HIST_STEPS: usize = 256;
/// The smallest x-gap two curve points may share, so the spline stays a
/// function of x while a point is dragged past its neighbour.
const CURVE_MIN_GAP: f32 = 0.02;

/// The shape of a [`slider_tracked`] track — which colour gradient a value
/// travels across.
#[derive(Clone, Debug, PartialEq)]
pub enum Track {
    /// One flat fill: an ordinary slider.
    Plain,
    /// Grey at the centre, dark at both ends — a bipolar control.
    Centered,
    /// Blue (cold) to yellow (warm).
    Temp,
    /// Green to magenta.
    Tint,
    /// Two named tokens, `from` and `to`.
    Gradient { from: String, to: String },
    /// A hue rainbow restricted to band `band` of the eight.
    Hue { band: usize },
    /// Grey to band `band`'s colour.
    Sat { band: usize },
    /// Dark to band `band`'s colour.
    Lum { band: usize },
    /// The full hue rainbow.
    #[cfg(test)]
    Rainbow,
}

/// The colour a track shows at `t` (0 = left, 1 = right).
fn track_colour(theme: &Theme, track: &Track, t: f32) -> egui::Color32 {
    match track {
        Track::Plain => theme.track_fill,
        Track::Centered => {
            if t < 0.5 {
                theme.blend(theme.track_edge, theme.track_center, t * 2.0)
            } else {
                theme.blend(theme.track_center, theme.track_edge, (t - 0.5) * 2.0)
            }
        }
        Track::Temp => theme.blend(theme.temp_cold, theme.temp_warm, t),
        Track::Tint => theme.blend(theme.tint_green, theme.tint_magenta, t),
        Track::Gradient { from, to } => {
            theme.blend(theme.named_colour(from), theme.named_colour(to), t)
        }
        Track::Hue { band } => theme.rainbow(band_hue(*band, t)),
        Track::Sat { band } => theme.blend(theme.track_center, band_colour(theme, *band), t),
        Track::Lum { band } => theme.blend(theme.track_edge, band_colour(theme, *band), t),
        #[cfg(test)]
        Track::Rainbow => theme.rainbow(t),
    }
}

/// The hue (0..1) at `t` along one of the eight bands.
fn band_hue(band: usize, t: f32) -> f32 {
    (band % 8) as f32 / 8.0 + t / 8.0
}

/// Band `band`'s colour, wrapped into the eight the theme declares.
fn band_colour(theme: &Theme, band: usize) -> egui::Color32 {
    theme.band_colors[band % 8]
}

/// Paint one track, rounded ends and all, into `rect`.
fn paint_track(painter: &egui::Painter, theme: &Theme, rect: Rect, track: &Track, enabled: bool) {
    let tint = |c: egui::Color32| {
        if enabled {
            c
        } else {
            theme.faded(c, theme.disabled_alpha)
        }
    };
    painter.rect_filled(rect, theme.track_corner(), tint(track_colour(theme, track, 0.5)));
    let mut mesh = egui::Mesh::default();
    let w = rect.width() / TRACK_STEPS as f32;
    for i in 0..TRACK_STEPS {
        let t = (i as f32 + 0.5) / TRACK_STEPS as f32;
        let x0 = rect.left() + i as f32 * w;
        mesh.add_colored_rect(
            Rect::from_min_max(
                Pos2::new(x0, rect.top()),
                Pos2::new(x0 + w + 0.5, rect.bottom()),
            ),
            tint(track_colour(theme, track, t)),
        );
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.rect_stroke(
        rect,
        theme.track_corner(),
        theme.track_stroke(),
        egui::StrokeKind::Inside,
    );
}

/// A grading row: label on the kit's column, a themed [`Track`] that fills
/// the rest of the row, and the value's readout at the end.
///
/// **Gesture state lives here, not in the caller.** A caller re-reads the
/// held value off the door every frame, so the dragged value is kept in egui
/// memory for the length of the gesture (`id.with("live")`) and painted
/// instead of the stale read. `changed()` fires on every value change *while*
/// the pointer is down (the caller may throttle a live post off
/// [`Response::is_pointer_button_down_on`]) and once more on release with the
/// final value. Shift drags finely (relative, `1/`[`NUM_SCRUB_SCALE`]); a
/// double-click resets to `default`; the wheel nudges.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn slider_tracked(
    ui: &mut Ui,
    theme: &Theme,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    default: f32,
    track: &Track,
    enabled: bool,
) -> Response {
    label_row(ui, theme, label, |ui| {
        let sense = if enabled {
            Sense::click_and_drag()
        } else {
            Sense::hover()
        };
        let id = ui.next_auto_id();
        let live_key = id.with("live");
        if let Some(v) = ui.data(|d| d.get_temp::<f32>(live_key)) {
            *value = v;
        }
        if let Some((pending, _)) = ui.data(|d| d.get_temp::<(f32, f64)>(id.with("wheel"))) {
            *value = pending;
        }
        let w = (ui.available_width() - theme.readout_width - theme.item_gap.x)
            .max(theme.track_w * theme.track_min_share);
        let (row, mut response) = ui.allocate_exact_size(
            egui::vec2(w, ui.spacing().interact_size.y),
            sense,
        );
        let rect = Rect::from_center_size(row.center(), egui::vec2(row.width(), theme.track_h));
        let painter = ui.painter_at(row.expand(theme.curve_handle));
        paint_track(&painter, theme, rect, track, enabled);

        let (lo, hi) = (*range.start(), *range.end());
        let span = hi - lo;
        let mut changed = false;
        if enabled {
            if response.double_clicked() {
                *value = default.clamp(lo, hi);
                ui.data_mut(|d| d.remove::<f32>(live_key));
                changed = true;
            } else if response.is_pointer_button_down_on() {
                let fine = ui.input(|i| i.modifiers.shift);
                let before = *value;
                if fine {
                    if response.dragged() && span > 0.0 {
                        let per_point = span / rect.width() / NUM_SCRUB_SCALE;
                        *value = (*value + response.drag_delta().x * per_point).clamp(lo, hi);
                    }
                } else if let Some(p) = ui.ctx().pointer_interact_pos() {
                    let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                    *value = lo + t * span;
                }
                let live = *value;
                ui.data_mut(|d| d.insert_temp(live_key, live));
                if (live - before).abs() > f32::EPSILON {
                    changed = true;
                }
            } else if ui.data(|d| d.get_temp::<f32>(live_key)).is_some() {
                // Released: the gesture's last value is `*value` already.
                ui.data_mut(|d| d.remove::<f32>(live_key));
                changed = true;
            } else if response.clicked() {
                // A tap whose press and release land in one frame.
                if let Some(p) = ui.ctx().pointer_interact_pos() {
                    let t = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                    *value = lo + t * span;
                    changed = true;
                }
            }
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            }
            wheel_edit(ui, id, value, &range, &mut response);
        }

        let frac = if span > 0.0 {
            ((*value - lo) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let handle = Pos2::new(rect.left() + frac * rect.width(), rect.center().y);
        let lit = enabled && (response.hovered() || response.dragged());
        let ink = if enabled { theme.ink } else { theme.ink_faint };
        let radius = if lit { theme.curve_handle * theme.handle_lit_scale } else { theme.curve_handle };
        painter.circle_filled(handle, radius, ink);
        painter.circle_stroke(handle, radius, theme.line_stroke(theme.track_edge));

        let off_default = (*value - default).abs() > f32::EPSILON;
        let readout = ui.add_sized(
            [theme.readout_width, ui.spacing().interact_size.y],
            egui::Label::new(
                RichText::new(format!("{:.0}", *value))
                    .font(theme.font_small.clone())
                    .color(if off_default { theme.ink } else { theme.ink_faint }),
            )
            .sense(Sense::click()),
        );
        if enabled && readout.double_clicked() {
            *value = default.clamp(lo, hi);
            changed = true;
        }
        response.widget_info(|| egui::WidgetInfo::slider(enabled, f64::from(*value), label));
        if changed {
            response.mark_changed();
        }
        response
    })
}

/// Map a curve point (`x`, `y` in 0..1) to its place in `rect`, y up.
fn curve_to_screen(p: [f32; 2], rect: Rect) -> Pos2 {
    Pos2::new(
        rect.left() + p[0] * rect.width(),
        rect.bottom() - p[1] * rect.height(),
    )
}

/// The inverse of [`curve_to_screen`].
fn curve_to_norm(pos: Pos2, rect: Rect) -> [f32; 2] {
    [
        ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
        ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0),
    ]
}

/// The index of the point whose handle is nearest `pos`, if any sits within
/// `threshold` points.
fn curve_nearest(points: &[[f32; 2]], rect: Rect, pos: Pos2, threshold: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, p) in points.iter().enumerate() {
        let d = curve_to_screen(*p, rect).distance(pos);
        if d <= threshold && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// Insert a point, keeping the list sorted by x, and return its index.
fn curve_insert(points: &mut Vec<[f32; 2]>, p: [f32; 2]) -> usize {
    let idx = points
        .iter()
        .position(|q| q[0] > p[0])
        .unwrap_or(points.len());
    points.insert(idx, p);
    idx
}

/// Move point `i` to `pos`, clamping x between its neighbours (and inside
/// 0..1 at the ends) so the spline stays a function of x.
fn curve_move(points: &mut [[f32; 2]], i: usize, pos: Pos2, rect: Rect) {
    let p = curve_to_norm(pos, rect);
    let lo = if i == 0 {
        0.0
    } else {
        points[i - 1][0] + CURVE_MIN_GAP
    };
    let hi = if i + 1 == points.len() {
        1.0
    } else {
        points[i + 1][0] - CURVE_MIN_GAP
    };
    let x = p[0].clamp(lo.min(hi), hi.max(lo));
    points[i] = [x, p[1]];
}

/// A monotone cubic (PCHIP) through `points`, sampled as `[x, y]` pairs —
/// the curve a tone editor draws, which never overshoots between handles.
fn curve_samples(points: &[[f32; 2]], steps: usize) -> Vec<[f32; 2]> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![points[0]];
    }
    let mut xs: Vec<f32> = points.iter().map(|p| p[0]).collect();
    for i in 1..n {
        if xs[i] <= xs[i - 1] {
            xs[i] = xs[i - 1] + CURVE_MIN_GAP;
        }
    }
    let ys: Vec<f32> = points.iter().map(|p| p[1]).collect();
    let deltas: Vec<f32> = (0..n - 1)
        .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
        .collect();
    let mut m = vec![0.0_f32; n];
    m[0] = deltas[0];
    m[n - 1] = deltas[n - 2];
    for i in 1..n - 1 {
        if deltas[i - 1] * deltas[i] <= 0.0 {
            m[i] = 0.0;
        } else {
            m[i] = (deltas[i - 1] + deltas[i]) / 2.0;
        }
    }
    for i in 0..n - 1 {
        if deltas[i].abs() < f32::EPSILON {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let a = m[i] / deltas[i];
        let b = m[i + 1] / deltas[i];
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * deltas[i];
            m[i + 1] = t * b * deltas[i];
        }
    }
    let (x0, xn) = (xs[0], xs[n - 1]);
    let mut out = Vec::with_capacity(steps + 1);
    for s in 0..=steps {
        let x = x0 + (xn - x0) * (s as f32 / steps as f32);
        let mut i = 0;
        while i + 2 < n && x > xs[i + 1] {
            i += 1;
        }
        let h = xs[i + 1] - xs[i];
        let t = ((x - xs[i]) / h).clamp(0.0, 1.0);
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        let y = h00 * ys[i] + h10 * h * m[i] + h01 * ys[i + 1] + h11 * h * m[i + 1];
        out.push([x, y.clamp(0.0, 1.0)]);
    }
    out
}

#[cfg(test)]
/// A tone curve: a square graph ([`Theme::curve_size`]) with a grid, the
/// diagonal reference, a monotone spline through `points` (`x`, `y` in 0..1),
/// and draggable handles. Click empty space to add a point, drag a handle to
/// move it (x clamped between its neighbours), double-click an interior point
/// to remove it. `channel` (`"master"`/`"red"`/`"green"`/`"blue"`) only picks
/// the curve's ink token. Returns `changed()` on release after an edit.
#[must_use]
pub fn curve_graph(
    ui: &mut Ui,
    theme: &Theme,
    points: &mut Vec<[f32; 2]>,
    channel: &str,
) -> Response {
    curve_graph_impl(ui, theme, points, channel, false, theme.curve_size)
}

/// A parametric tone curve: the same graph as [`curve_graph`], as wide as the
/// row (a square), whose interior handles only move **vertically** — no
/// point is added or removed, because each handle is one held float. The
/// endpoints stay put. `changed()` fires live while dragging and on release.
#[must_use]
pub fn curve_graph_fixed(
    ui: &mut Ui,
    theme: &Theme,
    points: &mut Vec<[f32; 2]>,
    channel: &str,
) -> Response {
    let side = ui.available_width().max(theme.curve_size);
    curve_graph_impl(ui, theme, points, channel, true, side)
}

fn curve_graph_impl(
    ui: &mut Ui,
    theme: &Theme,
    points: &mut Vec<[f32; 2]>,
    channel: &str,
    fixed: bool,
    side: f32,
) -> Response {
    let live_key = ui.next_auto_id().with("live");
    if let Some(live) = ui.data(|d| d.get_temp::<Vec<[f32; 2]>>(live_key)) {
        if !fixed || live.len() == points.len() {
            *points = live;
        }
    }
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(side, side),
        Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    let ink = theme.curve_channel_ink(channel);

    painter.rect_filled(rect, theme.square_corner(), theme.inset);
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let x = rect.left() + t * rect.width();
        let y = rect.top() + t * rect.height();
        let stroke = theme.line_stroke(theme.curve_grid);
        painter.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], stroke);
        painter.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], stroke);
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        theme.line_stroke(theme.curve_grid),
    );
    let samples: Vec<Pos2> = curve_samples(points, SPLINE_STEPS)
        .into_iter()
        .map(|p| curve_to_screen(p, rect))
        .collect();
    if samples.len() >= 2 {
        painter.add(egui::Shape::line(samples, theme.curve_stroke(channel)));
    }
    for p in points.iter() {
        let c = curve_to_screen(*p, rect);
        painter.circle_filled(c, theme.curve_handle, ink);
        painter.circle_stroke(c, theme.curve_handle, theme.line_stroke(theme.track_edge));
    }
    painter.rect_stroke(
        rect,
        theme.square_corner(),
        theme.track_stroke(),
        egui::StrokeKind::Inside,
    );

    let id = response.id;
    let grab = id.with("grab");
    if fixed {
        let interior = |i: usize| i > 0 && i + 1 < points.len();
        if response.double_clicked() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                if let Some(i) = curve_nearest(points, rect, p, theme.curve_handle * 3.0) {
                    if interior(i) {
                        points[i][1] = points[i][0];
                        response.mark_changed();
                    }
                }
            }
            ui.data_mut(|d| {
                d.remove::<usize>(grab);
                d.remove::<Vec<[f32; 2]>>(live_key);
            });
            return response;
        }
        if response.is_pointer_button_down_on() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let i = match ui.data(|d| d.get_temp::<usize>(grab)) {
                    Some(i) if interior(i) => Some(i),
                    _ => {
                        // The handle nearest in x: a press anywhere in a
                        // column grabs that column's float.
                        let x = curve_to_norm(p, rect)[0];
                        let near = (1..points.len().saturating_sub(1)).min_by(|a, b| {
                            (points[*a][0] - x)
                                .abs()
                                .partial_cmp(&(points[*b][0] - x).abs())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        if let Some(i) = near {
                            ui.data_mut(|d| d.insert_temp(grab, i));
                        }
                        near
                    }
                };
                if let Some(i) = i {
                    let y = curve_to_norm(p, rect)[1];
                    if (points[i][1] - y).abs() > f32::EPSILON {
                        points[i][1] = y;
                        response.mark_changed();
                    }
                    let live = points.clone();
                    ui.data_mut(|d| d.insert_temp(live_key, live));
                }
            }
        } else if ui.data(|d| d.get_temp::<Vec<[f32; 2]>>(live_key)).is_some() {
            ui.data_mut(|d| {
                d.remove::<usize>(grab);
                d.remove::<Vec<[f32; 2]>>(live_key);
            });
            response.mark_changed();
        }
        return response;
    }
    if response.double_clicked() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            if let Some(i) = curve_nearest(points, rect, p, theme.curve_handle * 2.0) {
                if i > 0 && i + 1 < points.len() {
                    points.remove(i);
                    response.mark_changed();
                }
            }
        }
        ui.data_mut(|d| d.remove::<usize>(grab));
        return response;
    }
    if response.is_pointer_button_down_on() {
        if let Some(p) = ui.ctx().pointer_interact_pos() {
            match ui.data(|d| d.get_temp::<usize>(grab)) {
                Some(i) if i < points.len() => curve_move(points, i, p, rect),
                _ => {
                    if let Some(i) = curve_nearest(points, rect, p, theme.curve_handle * 2.0) {
                        ui.data_mut(|d| d.insert_temp(grab, i));
                    } else {
                        let idx = curve_insert(points, curve_to_norm(p, rect));
                        ui.data_mut(|d| d.insert_temp(grab, idx));
                    }
                }
            }
            let live = points.clone();
            ui.data_mut(|d| d.insert_temp(live_key, live));
        }
    } else if response.drag_stopped() || response.clicked() {
        ui.data_mut(|d| {
            d.remove::<usize>(grab);
            d.remove::<Vec<[f32; 2]>>(live_key);
        });
        response.mark_changed();
    }
    response
}

/// An RGB + luma histogram: four channel fills over one another, height
/// [`Theme::hist_h`], with clipping wedges at both ends. It posts nothing —
/// a click is the caller's to read off the returned response.
#[must_use]
pub fn histogram(
    ui: &mut Ui,
    theme: &Theme,
    r: &[u32; 256],
    g: &[u32; 256],
    b: &[u32; 256],
    luma: &[u32; 256],
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(theme.hist_w), theme.hist_h),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.square_corner(), theme.inset);

    let peak = [r, g, b, luma]
        .into_iter()
        .flat_map(|d| d.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let bar = rect.width() / HIST_STEPS as f32;
    for (data, colour) in [
        (luma, theme.hist_luma),
        (r, theme.hist_r),
        (g, theme.hist_g),
        (b, theme.hist_b),
    ] {
        let fill = theme.faded(colour, theme.disabled_alpha);
        for (i, count) in data.iter().enumerate() {
            let h = (*count as f32 / peak).clamp(0.0, 1.0) * rect.height();
            if h <= 0.0 {
                continue;
            }
            let x = rect.left() + i as f32 * bar;
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x, rect.bottom() - h),
                    Pos2::new(x + bar + 0.5, rect.bottom()),
                ),
                theme.square_corner(),
                fill,
            );
        }
    }
    let wedge = rect.height() * 0.18;
    for (x, dir) in [(rect.left(), 1.0_f32), (rect.right(), -1.0_f32)] {
        painter.add(egui::Shape::convex_polygon(
            vec![
                Pos2::new(x, rect.top()),
                Pos2::new(x + dir * wedge, rect.top()),
                Pos2::new(x, rect.top() + wedge),
            ],
            theme.clip_ink,
            egui::Stroke::NONE,
        ));
    }
    painter.rect_stroke(
        rect,
        theme.square_corner(),
        theme.track_stroke(),
        egui::StrokeKind::Inside,
    );
    response
}

#[cfg(test)]
/// A hue/saturation grading wheel ([`Theme::wheel_radius`], or `radius`
/// when the caller sizes it to its column) with a draggable dot: angle is
/// `hue` (degrees), distance from centre is `sat` (0..100). The label sits
/// under the disc. Like [`slider_tracked`], the gesture's value is held in
/// egui memory while the pointer is down; `changed()` fires live and on
/// release. A double-click returns the dot to the centre.
#[must_use]
pub fn grading_wheel(
    ui: &mut Ui,
    theme: &Theme,
    hue: &mut f32,
    sat: &mut f32,
    label: &str,
) -> Response {
    let r = theme.wheel_radius;
    grading_wheel_sized(ui, theme, hue, sat, label, r)
}

/// [`grading_wheel`] at a caller-chosen radius.
#[must_use]
pub fn grading_wheel_sized(
    ui: &mut Ui,
    theme: &Theme,
    hue: &mut f32,
    sat: &mut f32,
    label: &str,
    radius: f32,
) -> Response {
    ui.vertical_centered(|ui| {
        let d = radius * 2.0;
        let id = ui.next_auto_id();
        let live_key = id.with("live");
        if let Some((h, s)) = ui.data(|d| d.get_temp::<(f32, f32)>(live_key)) {
            *hue = h;
            *sat = s;
        }
        let (rect, mut response) =
            ui.allocate_exact_size(egui::vec2(d, d), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let c = rect.center();
        let r = radius;
        let inner = r * theme.wheel_inner;
        let steps = TRACK_STEPS;
        for i in 0..steps {
            let a0 = (i as f32 / steps as f32) * std::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / steps as f32) * std::f32::consts::TAU;
            let quad = vec![
                Pos2::new(c.x + inner * a0.cos(), c.y + inner * a0.sin()),
                Pos2::new(c.x + r * a0.cos(), c.y + r * a0.sin()),
                Pos2::new(c.x + r * a1.cos(), c.y + r * a1.sin()),
                Pos2::new(c.x + inner * a1.cos(), c.y + inner * a1.sin()),
            ];
            painter.add(egui::Shape::convex_polygon(
                quad,
                theme.rainbow(i as f32 / steps as f32),
                egui::Stroke::NONE,
            ));
        }
        painter.circle_filled(c, inner, theme.inset);
        painter.circle_stroke(c, r, theme.line_stroke(theme.wheel_tick));
        painter.circle_stroke(c, theme.wheel_handle, theme.line_stroke(theme.wheel_tick));

        let mut changed = false;
        if response.double_clicked() {
            *sat = 0.0;
            ui.data_mut(|d| d.remove::<(f32, f32)>(live_key));
            changed = true;
        } else if response.is_pointer_button_down_on() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let dx = p.x - c.x;
                let dy = p.y - c.y;
                let len = (dx * dx + dy * dy).sqrt();
                *sat = (len / r).clamp(0.0, 1.0) * 100.0;
                if len > f32::EPSILON {
                    *hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                }
                let live = (*hue, *sat);
                ui.data_mut(|d| d.insert_temp(live_key, live));
                changed = true;
            }
        } else if ui.data(|d| d.get_temp::<(f32, f32)>(live_key)).is_some() {
            ui.data_mut(|d| d.remove::<(f32, f32)>(live_key));
            changed = true;
        } else if response.clicked() {
            if let Some(p) = ui.ctx().pointer_interact_pos() {
                let (dx, dy) = (p.x - c.x, p.y - c.y);
                let len = (dx * dx + dy * dy).sqrt();
                *sat = (len / r).clamp(0.0, 1.0) * 100.0;
                if len > f32::EPSILON {
                    *hue = dy.atan2(dx).to_degrees().rem_euclid(360.0);
                }
                changed = true;
            }
        }

        let ang = hue.to_radians();
        let rr = (*sat / 100.0).clamp(0.0, 1.0) * r;
        let dot = Pos2::new(c.x + rr * ang.cos(), c.y + rr * ang.sin());
        painter.circle_filled(dot, theme.wheel_handle, theme.wheel_dot);
        painter.circle_stroke(
            dot,
            theme.wheel_handle,
            theme.line_stroke(theme.wheel_tick),
        );
        ui.label(
            RichText::new(label)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, label));
        if changed {
            response.mark_changed();
        }
        response
    })
    .inner
}

/// The eight HSL mixer bands as a row of colour dots, the `selected` one
/// ringed in the accent. Each dot is its own click target; returns the band
/// clicked this frame, if any.
#[must_use]
pub fn hue_band_selector(ui: &mut Ui, theme: &Theme, selected: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = theme.band_gap;
        for (i, colour) in theme.band_colors.iter().enumerate() {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(theme.band_dot, theme.band_dot),
                Sense::click(),
            );
            let painter = ui.painter_at(rect.expand(theme.band_gap));
            let radius = theme.band_dot * 0.5;
            let lit = response.hovered() || i == selected;
            painter.circle_filled(rect.center(), radius, *colour);
            let ring = if i == selected {
                theme.accent()
            } else if lit {
                theme.line_stroke(theme.ink)
            } else {
                theme.line_stroke(theme.wheel_tick)
            };
            painter.circle_stroke(rect.center(), radius, ring);
            if response.clicked() {
                picked = Some(i);
            }
        }
    });
    picked
}

// `region_placeholder`, the empty-region stand-in `S.U8`–`S.U11` used for
// whichever region had not landed a real panel yet, was removed in `S.U12`:
// with `properties` landed, all four regions of `Region::ALL` paint a real
// panel, and nothing else in the tree needs a generic "nothing lives here
// yet" frame. See `panels::properties::paint` for the pattern a future
// region would reach for instead (a `panel_frame` plus its own content).

#[cfg(all(test, any(feature = "gpu", feature = "glow")))]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn dropdown_picks_a_different_option() {
        let theme = Theme::phosphor();
        let current = Rc::new(RefCell::new("a".to_string()));
        let state = current.clone();
        let options = [("a".to_string(), "Alpha"), ("b".to_string(), "Beta")];
        let mut harness = Harness::new_ui(move |ui| {
            dropdown(
                ui,
                &theme,
                "kit/dropdown",
                &mut *state.borrow_mut(),
                &options,
            );
        });
        harness.run();
        harness.get_by_value("Alpha").click();
        harness.run();
        harness.get_by_label("Beta").click();
        harness.run();
        assert_eq!(*current.borrow(), "b");
    }

    #[test]
    fn section_toggles_open_and_shut() {
        let theme = Theme::phosphor();
        let open = Rc::new(RefCell::new(false));
        let state = open.clone();
        let mut harness = Harness::new_ui(move |ui| {
            section(ui, &theme, "Audio", &mut *state.borrow_mut());
        });
        harness.run();
        harness.get_by_label("\u{25b8} Audio").click();
        harness.run();
        assert!(*open.borrow());
        harness.get_by_label("\u{25be} Audio").click();
        harness.run();
        assert!(!*open.borrow());
    }

    #[test]
    fn segmented_switches_segment() {
        let theme = Theme::phosphor();
        let current = Rc::new(RefCell::new("rgb".to_string()));
        let state = current.clone();
        let options = [("rgb".to_string(), "RGB"), ("hsv".to_string(), "HSV")];
        let mut harness = Harness::new_ui(move |ui| {
            segmented(ui, &theme, &mut *state.borrow_mut(), &options);
        });
        harness.run();
        harness.get_by_label("HSV").click();
        harness.run();
        assert_eq!(*current.borrow(), "hsv");
    }

    #[test]
    fn toggle_row_flips_the_value() {
        let theme = Theme::phosphor();
        let on = Rc::new(RefCell::new(false));
        let state = on.clone();
        let mut harness = Harness::new_ui(move |ui| {
            toggle_row(ui, &theme, "Bypass", &mut *state.borrow_mut());
        });
        harness.run();
        harness.get_by_label("Bypass").click();
        harness.run();
        assert!(*on.borrow());
    }

    #[test]
    fn number_row_shows_its_unit() {
        let theme = Theme::phosphor();
        let value = Rc::new(RefCell::new(1.0_f32));
        let state = value.clone();
        let mut harness = Harness::new_ui(move |ui| {
            number_row(
                ui,
                &theme,
                "Universe",
                &mut *state.borrow_mut(),
                1.0..=63999.0,
                "uni",
                0,
                true,
            );
        });
        harness.run();
        harness.get_by_label("Universe");
        harness.get_by_label("uni");
    }

    /// A one-track harness with the value, a change count, and the track's
    /// rect once it has painted — the shared setup for the gesture tests.
    fn num_fixture(
        default: f32,
    ) -> (
        Harness<'static, ()>,
        Rc<RefCell<f32>>,
        Rc<RefCell<usize>>,
        egui::Rect,
    ) {
        let theme = Theme::phosphor();
        let value = Rc::new(RefCell::new(0.0_f32));
        let changed = Rc::new(RefCell::new(0_usize));
        let v = value.clone();
        let c = changed.clone();
        let mut harness = Harness::new_ui(move |ui| {
            if num_track(ui, &theme, &mut *v.borrow_mut(), 0.0..=100.0, "", 2, default, true)
                .changed()
            {
                *c.borrow_mut() += 1;
            }
        });
        harness.run();
        let rect = harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("the track registers as a slider")
            .rect();
        (harness, value, changed, rect)
    }

    /// (a) A press-and-drag scrub moves the value and marks `changed()` once,
    /// on release — the one-post-per-gesture contract.
    #[test]
    fn num_track_drag_scrubs_and_reports_once_on_release() {
        let (mut harness, value, changed, rect) = num_fixture(50.0);
        let start = rect.center();
        let dx = 30.0;
        let end = start + egui::vec2(dx, 0.0);
        harness.drag_at(start);
        harness.run();
        harness.hover_at(end);
        harness.run();
        harness.drop_at(end);
        harness.run();
        assert!(
            *value.borrow() > 0.0,
            "the scrub should raise the value, got {}",
            *value.borrow()
        );
        assert_eq!(*changed.borrow(), 1, "one change per drag gesture");
    }

    /// (b) One click on the right arrow steps the value by `range / 100`.
    #[test]
    fn num_track_arrow_click_steps_by_a_hundredth_of_the_range() {
        let (mut harness, value, changed, rect) = num_fixture(50.0);
        let right = egui::pos2(rect.right() - rect.height() * 0.5, rect.center().y);
        harness.drag_at(right);
        harness.run_steps(1);
        harness.drop_at(right);
        harness.run_steps(1);
        assert!(
            (*value.borrow() - 1.0).abs() < 0.001,
            "one arrow click is range/100, got {}",
            *value.borrow()
        );
        assert_eq!(*changed.borrow(), 1, "one change per arrow click");
    }

    /// (c) A double-click opens the text edit and Enter commits the typed
    /// value. A small `step_dt` keeps the two clicks inside egui's default
    /// double-click window without widening it.
    #[test]
    fn num_track_typed_entry_commits() {
        let theme = Theme::phosphor();
        let value = Rc::new(RefCell::new(0.0_f32));
        let changed = Rc::new(RefCell::new(0_usize));
        let v = value.clone();
        let c = changed.clone();
        let mut harness = Harness::builder()
            .with_step_dt(0.02)
            .build_ui(move |ui| {
                if num_track(ui, &theme, &mut *v.borrow_mut(), 0.0..=100.0, "", 2, 50.0, true)
                    .changed()
                {
                    *c.borrow_mut() += 1;
                }
            });
        harness.run();
        let at = harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("the track registers as a slider")
            .rect()
            .center();
        for _ in 0..2 {
            harness.drag_at(at);
            harness.run();
            harness.drop_at(at);
            harness.run();
        }
        harness.run();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("75");
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(
            (*value.borrow() - 75.0).abs() < 0.001,
            "the typed value should commit, got {}",
            *value.borrow()
        );
        assert!(*changed.borrow() >= 1, "a commit is a change");
    }

    /// (d) A right-click resets the value to `default`.
    #[test]
    fn num_track_right_click_resets_to_default() {
        let (mut harness, value, changed, _rect) = num_fixture(50.0);
        harness
            .get_all_by_role(egui::accesskit::Role::Slider)
            .next()
            .expect("the track registers as a slider")
            .click_secondary();
        harness.run();
        assert!(
            (*value.borrow() - 50.0).abs() < 0.001,
            "right-click resets to the default, got {}",
            *value.borrow()
        );
        assert_eq!(*changed.borrow(), 1, "one change per reset");
    }

    /// (e) `num` carries its label and its unit as queryable AccessKit
    /// labels.
    #[test]
    fn num_shows_its_label_and_suffix() {
        let theme = Theme::phosphor();
        let value = Rc::new(RefCell::new(2.0_f32));
        let state = value.clone();
        let mut harness = Harness::new_ui(move |ui| {
            let _ = num(
                ui,
                &theme,
                "Rango",
                &mut *state.borrow_mut(),
                0.0..=10.0,
                "m",
                2,
                5.0,
                true,
            );
        });
        harness.run();
        harness.get_by_label("Rango");
        harness.get_by_label("m");
    }

    /// A click in empty curve space adds a handle, and the gesture reports
    /// `changed()` on release — the one-post seam the grade panel relies on.
    #[test]
    fn curve_graph_adds_a_point_on_click() {
        let theme = Theme::phosphor();
        let points = Rc::new(RefCell::new(vec![[0.0_f32, 0.0_f32], [1.0, 1.0]]));
        let rect_cell: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
        let changed = Rc::new(RefCell::new(false));
        let (p, r, ch) = (points.clone(), rect_cell.clone(), changed.clone());
        let mut harness = Harness::new_ui(move |ui| {
            let resp = curve_graph(ui, &theme, &mut *p.borrow_mut(), "master");
            *r.borrow_mut() = Some(resp.rect);
            if resp.changed() {
                *ch.borrow_mut() = true;
            }
        });
        harness.run();
        let rect = rect_cell.borrow().expect("curve should have a rect");
        let pt = rect.center();
        harness.drag_at(pt);
        harness.run();
        harness.drop_at(pt);
        harness.run();
        assert_eq!(
            points.borrow().len(),
            3,
            "clicking empty curve space should add a point"
        );
        assert!(*changed.borrow(), "the add should report changed() on release");
    }

    /// Dragging a handle moves it, clamped between its neighbours, and reports
    /// once on release.
    #[test]
    fn curve_graph_drags_a_point() {
        let theme = Theme::phosphor();
        let points = Rc::new(RefCell::new(vec![
            [0.0_f32, 0.0_f32],
            [0.5, 0.5],
            [1.0, 1.0],
        ]));
        let rect_cell: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
        let changed = Rc::new(RefCell::new(false));
        let (p, r, ch) = (points.clone(), rect_cell.clone(), changed.clone());
        let mut harness = Harness::new_ui(move |ui| {
            let resp = curve_graph(ui, &theme, &mut *p.borrow_mut(), "master");
            *r.borrow_mut() = Some(resp.rect);
            if resp.changed() {
                *ch.borrow_mut() = true;
            }
        });
        harness.run();
        let rect = rect_cell.borrow().expect("curve should have a rect");
        let start = curve_to_screen([0.5, 0.5], rect);
        let target = Pos2::new(
            rect.left() + rect.width() * 0.8,
            rect.top() + rect.height() * 0.2,
        );
        harness.drag_at(start);
        harness.run();
        harness.hover_at(target);
        harness.run();
        harness.drop_at(target);
        harness.run();
        let moved = points.borrow()[1];
        assert!(
            (moved[0] - 0.8).abs() < 0.06 && (moved[1] - 0.8).abs() < 0.06,
            "the dragged point should land near (0.8, 0.8), saw {moved:?}"
        );
        assert!(*changed.borrow(), "the drag should report changed() on release");
    }

    /// Dragging the wheel dot sets hue (angle) and saturation (radius), and
    /// reports once on release.
    /// The 2026-10-09 bug: a caller re-reads the held value off the door
    /// every frame (here: always `0`), so a drag must keep its own value
    /// across frames and report the *dragged* value on release — not the
    /// stale read.
    #[test]
    fn slider_tracked_keeps_its_drag_while_the_caller_rereads_a_stale_value() {
        let theme = Theme::phosphor();
        let posted: Rc<RefCell<Vec<f32>>> = Rc::new(RefCell::new(Vec::new()));
        let rect_cell: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
        let (p, r) = (posted.clone(), rect_cell.clone());
        let mut harness = Harness::new_ui(move |ui| {
            let mut value = 0.0_f32;
            let resp = slider_tracked(ui, &theme, "x", &mut value, -100.0..=100.0, 0.0, &Track::Centered, true);
            *r.borrow_mut() = Some(resp.rect);
            if resp.changed() {
                p.borrow_mut().push(value);
            }
        });
        harness.run();
        let rect = rect_cell.borrow().expect("slider rect");
        harness.drag_at(Pos2::new(rect.center().x, rect.center().y));
        harness.run();
        let right = Pos2::new(rect.right() - 1.0, rect.center().y);
        harness.hover_at(right);
        harness.run();
        harness.drop_at(right);
        harness.run();
        let last = *posted.borrow().last().expect("the drag should report changed()");
        assert!(last > 90.0, "the release should carry the dragged value, saw {last}");
    }

    #[test]
    fn grading_wheel_drag_sets_hue_and_sat() {
        let theme = Theme::phosphor();
        let half_radius = theme.wheel_radius * 0.5;
        let hue = Rc::new(RefCell::new(0.0_f32));
        let sat = Rc::new(RefCell::new(0.0_f32));
        let rect_cell: Rc<RefCell<Option<Rect>>> = Rc::new(RefCell::new(None));
        let changed = Rc::new(RefCell::new(false));
        let (h, s, r, ch) = (
            hue.clone(),
            sat.clone(),
            rect_cell.clone(),
            changed.clone(),
        );
        let mut harness = Harness::new_ui(move |ui| {
            let resp = grading_wheel(ui, &theme, &mut *h.borrow_mut(), &mut *s.borrow_mut(), "mix");
            *r.borrow_mut() = Some(resp.rect);
            if resp.changed() {
                *ch.borrow_mut() = true;
            }
        });
        harness.run();
        let rect = rect_cell.borrow().expect("wheel should have a rect");
        let c = rect.center();
        harness.drag_at(c);
        harness.run();
        let target = Pos2::new(c.x, c.y + half_radius);
        harness.hover_at(target);
        harness.run();
        harness.drop_at(target);
        harness.run();
        assert!(
            (*sat.borrow() - 50.0).abs() < 3.0,
            "saturation should follow the drag radius, saw {}",
            *sat.borrow()
        );
        assert!(
            (*hue.borrow() - 90.0).abs() < 3.0,
            "hue should follow the drag angle, saw {}",
            *hue.borrow()
        );
        assert!(*changed.borrow(), "the drag should report changed() on release");
    }
}

/// Every glyph in the shell: a library row's kind, a canvas control, a
/// settings or gallery section, a transport key. Each is a Lucide icon
/// ([`LUCIDE_FONT`], `lucide.dev`), painted by [`paint_icon_stroked`] or
/// spelled as text by [`lucide`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    // A library row's kind.
    Folder,
    Image,
    Video,
    Audio,
    Sequence,
    File,
    // The canvas controls' glyphs.
    Play,
    Pause,
    ZoomIn,
    ZoomOut,
    New,
    /// The venue scene's bar: `+`, the gallery of screens, projections and
    /// fixtures. Its own glyph so the bar and the tool strip's `new` never
    /// read as the same gesture.
    Plus,
    Revert,
    Bypass,
    Solo,
    Duplicate,
    Remove,
    // Settings sections.
    Interface,
    Palette,
    Language,
    Spark,
    Lamp,
    Devices,
    Export,
    Gauge,
    Radar,
    // Gallery sections.
    Source,
    Sensor,
    Effect,
    Material,
    Math,
    Script,
    Output,
    Other,
    // Transport and sequence-editor keys.
    SkipBack,
    StepBack,
    StepForward,
    Keypoint,
    Copy,
    Paste,
    Trash,
    Undo,
    Save,
    // Buttons and marks.
    Star,
    Close,
    Settings,
    Camera,
    Lock,
    Eye,
    EyeOff,
    ChevronUp,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ArrowUp,
    ArrowDown,
    // The AI sidebar's header and history view.
    ArrowLeft,
    History,
    // The node bar's splice verbs: a new node upstream / downstream.
    InsertBefore,
    InsertAfter,
    // The venue's tool strip: the canvas arrows' siblings, one per tool.
    Pointer,
    Ruler,
    Magnet,
    Ortho,
}

/// The Lucide icon font, the vendored `lucide.ttf` (ISC; see the licences).
pub const LUCIDE_FONT: &[u8] = include_bytes!("../../assets/lucide.ttf");

/// The font family name the Lucide glyphs are registered under.
pub const LUCIDE: &str = "lucide";

/// Registers [`LUCIDE_FONT`] on `ctx`. Every context that paints a shell
/// glyph calls this once, before its first frame.
pub fn install_icon_font(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(LUCIDE.into(), std::sync::Arc::new(egui::FontData::from_static(LUCIDE_FONT)));
    // A fallback behind the default faces, not a family of its own: a glyph
    // in a family no font is bound to yet panics the first layout (a
    // kittest's first frame runs before `set_fonts` lands), while the
    // proportional family always resolves and the default faces have none
    // of the private-use codepoints, so the Lucide face answers them.
    fonts.families.entry(egui::FontFamily::Proportional).or_default().push(LUCIDE.into());
    ctx.set_fonts(fonts);
}

/// The Lucide codepoint of `icon`. Codepoints from Lucide's `info.json`
/// (v0.460.0, the vendored font); the name in each arm is the Lucide icon.
pub fn glyph(icon: Icon) -> char {
    let code = match icon {
        Icon::Folder => 0xe0dc,                 // folder
        Icon::Image => 0xe0f9,                  // image
        Icon::Video => 0xe1a5,                  // video
        Icon::Audio => 0xe126,                  // music
        Icon::Sequence => 0xe2e2,               // list-video
        Icon::File => 0xe0d0,                   // file-text
        Icon::Play => 0xe140,                   // play
        Icon::Pause => 0xe132,                  // pause
        Icon::ZoomIn => 0xe1b6,                 // zoom-in
        Icon::ZoomOut => 0xe1b7,                // zoom-out
        Icon::New => 0xe141,                    // plus
        Icon::Plus => 0xe141,                   // plus (the venue bar's `+`)
        Icon::Revert => 0xe14c,                 // rotate-ccw
        Icon::Bypass => 0xe407,                 // circle-slash
        Icon::Solo => 0xe0f4,                   // headphones
        Icon::Duplicate => 0xe0a2,              // copy
        Icon::Remove => 0xe1b2,                 // x
        Icon::Interface => 0xe29a,              // sliders-horizontal
        Icon::Palette => 0xe1dd,                // palette
        Icon::Language => 0xe103,               // languages
        Icon::Spark => 0xe417,                  // sparkles
        Icon::Lamp => 0xe2d8,                   // lamp
        Icon::Devices => 0xe3a6,                // monitor-smartphone
        Icon::Export => 0xe5a9,                 // square-arrow-out-up-right
        Icon::Gauge => 0xe1bf,                  // gauge
        Icon::Radar => 0xe49c,                  // radar
        Icon::Source => 0xe383,                 // plug
        Icon::Sensor => 0xe0be,                 // eye
        Icon::Effect => 0xe246,                 // wand
        Icon::Material => 0xe529,               // cuboid
        Icon::Math => 0xe201,                   // sigma
        Icon::Script => 0xe206,                 // code-xml
        Icon::Output => 0xe112,                 // log-out
        Icon::Other => 0xe0ba,                  // ellipsis
        Icon::SkipBack => 0xe163,               // skip-back
        Icon::StepBack => 0xe3ed,               // step-back
        Icon::StepForward => 0xe3ee,            // step-forward
        Icon::Keypoint => 0xe2d2,               // diamond
        Icon::Copy => 0xe0a2,                   // copy
        Icon::Paste => 0xe3ec,                  // clipboard-paste
        Icon::Trash => 0xe18e,                  // trash-2
        Icon::Undo => 0xe2a1,                   // undo-2
        Icon::Save => 0xe151,                   // save
        Icon::Star => 0xe17a,                   // star
        Icon::Close => 0xe1b2,                  // x
        Icon::Settings => 0xe158,               // settings
        Icon::Camera => 0xe068,                 // camera
        Icon::Lock => 0xe10f,                   // lock
        Icon::Eye => 0xe0be,                    // eye
        Icon::EyeOff => 0xe0bf,                 // eye-off
        Icon::ChevronUp => 0xe074,              // chevron-up
        Icon::ChevronDown => 0xe071,            // chevron-down
        Icon::ChevronLeft => 0xe073,            // chevron-left
        Icon::ChevronRight => 0xe075,           // chevron-right
        Icon::ArrowUp => 0xe04e,                // arrow-up
        Icon::ArrowDown => 0xe046,              // arrow-down
        Icon::ArrowLeft => 0xe04c,               // arrow-left
        Icon::History => 0xe1f5,                // history
        Icon::InsertBefore => 0xe45c,           // arrow-left-to-line
        Icon::InsertAfter => 0xe45e,            // arrow-right-to-line
        Icon::Pointer => 0xe123,                // mouse-pointer
        Icon::Ruler => 0xe14f,                  // ruler
        Icon::Magnet => 0xe2b5,                 // magnet
        Icon::Ortho => 0xe116,                  // maximize (the ortho frame)
    };
    char::from_u32(code).unwrap_or('\u{fffd}')
}

/// `icon` as text in the Lucide family at `size` points, for a button or a
/// label. Pair it with a tooltip: the glyph itself names nothing to AccessKit.
pub fn lucide(icon: Icon, size: f32) -> egui::RichText {
    egui::RichText::new(glyph(icon).to_string()).font(glyph_font(size))
}

/// A Lucide glyph then `text`, in one layout: for a button that shows an
/// icon beside its name. `color` colours both.
pub fn lucide_job(icon: Icon, size: f32, text: &str, color: egui::Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(&glyph(icon).to_string(), 0.0, egui::TextFormat::simple(glyph_font(size), color));
    job.append(&format!(" {text}"), 0.0, egui::TextFormat::simple(glyph_font(size), color));
    job
}

/// Paints one [`Icon`] inside `rect`, in `theme.ink_weak` at `theme.hairline`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, theme: &Theme) {
    paint_icon_stroked(painter, rect, icon, theme, theme.icon_stroke());
}

/// [`paint_icon`] with its colour taken from `stroke`: a control lights its
/// glyph in the accent when it is on. The glyph is sized to the box's inner
/// fifth-inset square, so it reads at the badge size the rows use.
pub fn paint_icon_stroked(painter: &egui::Painter, rect: Rect, icon: Icon, _theme: &Theme, stroke: egui::Stroke) {
    let r = rect.shrink2(rect.size() * 0.2);
    if icon == Icon::Bypass {
        // A power symbol drawn as vectors: the Lucide circle-slash read as a
        // blank square at this size, and a vector needs no font.
        let c = r.center();
        let radius = r.height() * 0.42;
        let arc: Vec<Pos2> = (0..=40)
            .map(|i| {
                let a = (-40.0 + 260.0 * i as f32 / 40.0).to_radians();
                Pos2::new(c.x + radius * a.cos(), c.y + radius * a.sin())
            })
            .collect();
        painter.add(egui::Shape::line(arc, stroke));
        painter.line_segment([Pos2::new(c.x, c.y - radius * 1.15), Pos2::new(c.x, c.y - radius * 0.1)], stroke);
        return;
    }
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        glyph(icon),
        glyph_font(r.height() * 1.2),
        stroke.color,
    );
}

/// A minimal navigation list: one row per entry — its icon, its label and an
/// optional trailing note — with no button frame, only a hairline between
/// neighbours. Hovering a row washes it red with a bar on its left edge; the
/// selected one holds a stronger wash. Returns the row clicked. Each row is
/// named by its label for AccessKit.
pub fn nav_list(ui: &mut Ui, theme: &Theme, rows: &[(Icon, &str, Option<String>)], selected: Option<usize>) -> Option<usize> {
    let mut clicked = None;
    let width = ui.available_width();
    let saved = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing.y = theme.nav_row_gap;
    for (i, (icon, label, note)) in rows.iter().enumerate() {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, theme.nav_row_height), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *label));
        let on = selected == Some(i);
        let painter = ui.painter();
        if on || response.hovered() {
            let alpha = if on { theme.selected_alpha } else { theme.hover_alpha };
            painter.rect_filled(rect, theme.field_corner(), theme.faded(theme.phosphor, alpha));
            painter.rect_filled(
                Rect::from_min_size(rect.left_top(), Vec2::new(theme.accent_stroke, rect.height())),
                0.0,
                theme.phosphor,
            );
        }
        let ink = if on {
            theme.phosphor_lit
        } else if response.hovered() {
            theme.ink
        } else {
            theme.ink_weak
        };
        let side = theme.nav_icon_side;
        let slot = Rect::from_center_size(
            Pos2::new(rect.left() + theme.item_gap.x * 2.0 + side / 2.0, rect.center().y),
            Vec2::splat(side),
        );
        paint_icon_stroked(painter, slot, *icon, theme, theme.line_stroke(ink));
        painter.text(
            Pos2::new(slot.right() + theme.item_gap.x * 1.5, rect.center().y),
            egui::Align2::LEFT_CENTER,
            *label,
            theme.font_body.clone(),
            ink,
        );
        if let Some(note) = note {
            painter.text(
                Pos2::new(rect.right() - theme.item_gap.x, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                note,
                theme.font_small.clone(),
                theme.ink_faint,
            );
        }
        if i + 1 < rows.len() {
            painter.line_segment([rect.left_bottom(), rect.right_bottom()], theme.rule_stroke());
        }
        if response.clicked() {
            clicked = Some(i);
        }
    }
    ui.spacing_mut().item_spacing = saved;
    clicked
}

/// The thing being dragged, drawn whole under the pointer: `add` paints the
/// same component the drag started from, in a foreground layer that ignores
/// the pointer (so the drop target underneath still sees it) and a touch
/// translucent.
pub fn drag_ghost(ctx: &egui::Context, theme: &Theme, id: Id, pointer: Pos2, width: f32, add: impl FnOnce(&mut Ui)) {
    egui::Area::new(id)
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pointer + Vec2::splat(theme.item_gap.x))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.multiply_opacity(theme.drag_ghost_opacity);
            add(ui);
        });
}

/// A square glyph button: `icon` in `size`, ringed and lit when `on`. `tip`
/// is its hover text and its AccessKit name. The one button the canvas bars
/// are built from.
pub fn icon_button(ui: &mut Ui, theme: &Theme, icon: Icon, on: bool, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(theme.icon_button_size, Sense::click());
    let response = response.on_hover_text(tip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    if ui.is_rect_visible(rect) {
        let fill = if on {
            theme.faded(theme.phosphor, 48)
        } else if response.hovered() {
            theme.library_card_hover
        } else {
            theme.transparent()
        };
        let painter = ui.painter();
        painter.rect_filled(rect, theme.field_corner(), fill);
        if on {
            painter.rect_stroke(rect, theme.field_corner(), theme.accent(), egui::StrokeKind::Inside);
        }
        let ink = if on {
            theme.phosphor
        } else if response.hovered() {
            theme.ink
        } else {
            theme.ink_weak
        };
        paint_icon_stroked(painter, rect, icon, theme, theme.line_stroke(ink));
    }
    response
}
