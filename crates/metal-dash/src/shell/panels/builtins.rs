//! Shared grid-cell paint helpers: the palette colour strip and the one fader
//! (grid cell and controller editor rows alike). The colour built-ins that
//! used to live here are gone — those buttons come from the creation pipeline.

use egui::{Rect, Response, Sense, Ui};

use crate::shell::classes::{rgb_color, ControlState, Theme};
use crate::shell::components;

/// The four stops of a palette built-in, as a strip along the foot of its
/// cell — `preset` indexes [`Theme::colour_presets`]. Shared with a palette
/// node's controller cell ([`super::lanes`]).
pub(crate) fn paint_strip(ui: &mut Ui, theme: &Theme, preset: usize, rect: Rect) {
    paint_stops(ui, theme, &theme.colour_presets[preset], rect);
}

/// `stops` as a colour strip along the foot of `rect` — the palette look
/// [paint_strip] draws, generalised so a palette **node** paints its own
/// `config.colors` and not only a stored preset. An empty list paints
/// nothing.
pub(crate) fn paint_stops(ui: &mut Ui, theme: &Theme, stops: &[[f32; 3]], rect: Rect) {
    if stops.is_empty() {
        return;
    }
    let strip = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - rect.height() * theme.swatch_strip),
        rect.max,
    );
    let width = strip.width() / stops.len() as f32;
    for (k, stop) in stops.iter().enumerate() {
        let x = strip.left() + width * k as f32;
        let part = Rect::from_min_max(egui::pos2(x, strip.top()), egui::pos2(x + width, strip.bottom()));
        ui.painter().rect_filled(part, theme.corner_radius(), rgb_color(*stop));
    }
}

/// One fader's gesture: the response it left behind and the `0..=1` value it
/// now reads — see [`fader`].
pub(crate) struct Fader {
    pub response: Response,
    pub value: f32,
}

/// One fader filling `rect` and running its drag/click gesture — the one
/// fader the grid's HSB built-ins and a controller editor's parameter rows
/// both reach, so there is a single implementation, not a second dial.
///
/// `value` is a fraction of the range (`0..=1`); the caller maps its own
/// `min`/`max` in and out. `horizontal` puts the value's fill on the x axis
/// (an editor row) instead of the y (a grid cell). The returned `value` is
/// the value after this frame's gesture; the caller clamps and posts. This
/// fn never talks to the door.
pub(crate) fn fader(
    ui: &mut Ui,
    theme: &Theme,
    id: egui::Id,
    label: &str,
    value: f32,
    rect: Rect,
    horizontal: bool,
    sense: Sense,
) -> Fader {
    let enabled = sense != Sense::hover();
    let mut response = ui.interact(rect, id, sense);
    let mut value = value;
    if enabled && (response.dragged() || response.clicked()) {
        if let Some(p) = response.interact_pointer_pos() {
            let span = if horizontal { rect.width() } else { rect.height() }.max(1.0);
            let at = if horizontal { p.x - rect.left() } else { rect.bottom() - p.y };
            value = (at / span).clamp(0.0, 1.0);
        }
    }
    if enabled {
        components::track_gestures(ui, id, &mut value, &(0.0..=1.0), 0.0, &mut response);
    }
    let state = ControlState::of(&response, enabled, false, false);
    components::value_track(ui, theme, rect, value, horizontal, state);
    let painter = ui.painter();
    if !label.is_empty() {
        let (pos, align) = if horizontal {
            (egui::pos2(rect.left() + theme.pad as f32, rect.center().y), egui::Align2::LEFT_CENTER)
        } else {
            (rect.center_top(), egui::Align2::CENTER_TOP)
        };
        painter.text(pos, align, label, theme.font_small.clone(), theme.control_ink(state));
    }
    Fader { response, value }
}
