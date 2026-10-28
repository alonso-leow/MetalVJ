//! The control surface's own cell faces — the 25-key piano and the knob —
//! and the two channels that carry a control cell's intent to the library
//! panel and the library's pick back to the grid.
//!
//! The grid (`lanes.rs`) owns placement, gestures and the door; this module
//! only paints a face inside the rect it is given and answers what the
//! pointer did on it.

use egui::{Align2, Id, Pos2, Rect, Response, Sense, Ui};

use crate::shell::classes::Theme;
use crate::surface::{LibraryIntent, PIANO_KEYS};

/// What the pointer did on a piano this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEdge {
    Press(u8),
    Release(u8),
}

/// Which note sits under `p` on a piano of `PIANO_KEYS` keys from `base`
/// painted in `rect`: black keys (the upper part) win over the white ones
/// beneath them.
pub fn note_at(rect: Rect, base: u8, black_depth: f32, p: Pos2) -> Option<u8> {
    if !rect.contains(p) {
        return None;
    }
    let notes: Vec<u8> = (0..PIANO_KEYS).map(|i| base.saturating_add(i)).collect();
    let whites: Vec<u8> = notes.iter().copied().filter(|n| !metal_midi::is_black(*n)).collect();
    let key_w = rect.width() / whites.len().max(1) as f32;
    if p.y <= rect.top() + rect.height() * black_depth {
        for &n in notes.iter().filter(|n| metal_midi::is_black(**n)) {
            if black_rect(rect, &whites, n, key_w, black_depth).is_some_and(|r| r.contains(p)) {
                return Some(n);
            }
        }
    }
    let i = (((p.x - rect.left()) / key_w).floor() as usize).min(whites.len().saturating_sub(1));
    whites.get(i).copied()
}

fn black_rect(rect: Rect, whites: &[u8], n: u8, key_w: f32, depth: f32) -> Option<Rect> {
    // A black key straddles the boundary after the white key a semitone below.
    let left_white = whites.iter().position(|&w| w + 1 == n)?;
    let cx = rect.left() + key_w * (left_white as f32 + 1.0);
    let w = key_w * 0.6;
    Some(Rect::from_min_max(
        Pos2::new(cx - w / 2.0, rect.top()),
        Pos2::new(cx + w / 2.0, rect.top() + rect.height() * depth),
    ))
}

/// Paints a piano in `rect` — lit keys in phosphor, `C` keys named — and
/// answers the press/release the pointer made on it (`held` is the note the
/// pointer is down on, kept by the caller between frames).
pub fn piano(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    base: u8,
    lit: &dyn Fn(u8) -> bool,
    held: &mut Option<u8>,
) -> (Response, Vec<KeyEdge>) {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let depth = theme.piano_black_depth;
    let notes: Vec<u8> = (0..PIANO_KEYS).map(|i| base.saturating_add(i)).collect();
    let whites: Vec<u8> = notes.iter().copied().filter(|n| !metal_midi::is_black(*n)).collect();
    let key_w = rect.width() / whites.len().max(1) as f32;
    for (i, &n) in whites.iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(rect.left() + key_w * i as f32, rect.top()),
            egui::vec2(key_w, rect.height()),
        )
        .shrink(theme.hairline);
        let fill = if lit(n) { theme.phosphor } else { theme.ink };
        painter.rect_filled(r, theme.square_corner(), fill);
        if n % 12 == 0 {
            painter.text(
                Pos2::new(r.center().x, r.bottom() - theme.hairline),
                Align2::CENTER_BOTTOM,
                metal_midi::note_name(n),
                theme.font_small.clone(),
                theme.inset,
            );
        }
    }
    for &n in notes.iter().filter(|n| metal_midi::is_black(**n)) {
        if let Some(r) = black_rect(rect, &whites, n, key_w, depth) {
            let fill = if lit(n) { theme.phosphor } else { theme.inset };
            painter.rect_filled(r, theme.square_corner(), fill);
        }
    }
    let mut edges = Vec::new();
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let down = response.is_pointer_button_down_on();
    let under = pointer.and_then(|p| note_at(rect, base, depth, p));
    let now = if down { under } else { None };
    if now != *held {
        if let Some(n) = held.take() {
            edges.push(KeyEdge::Release(n));
        }
        if let Some(n) = now {
            edges.push(KeyEdge::Press(n));
        }
        *held = now;
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "piano"));
    (response, edges)
}

/// Paints a knob in `rect` — an arc whose sweep is `frac` (none when `None`),
/// the label under it, `value` in its middle — and answers the whole steps a
/// vertical drag or the wheel made (`acc` carries a part-step between frames).
pub fn dial(
    ui: &mut Ui,
    theme: &Theme,
    id: Id,
    rect: Rect,
    label: &str,
    value: &str,
    frac: Option<f32>,
    lit: bool,
    acc: &mut f32,
) -> (Response, i32) {
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.corner_radius(), if lit { theme.accent_vivid } else { theme.inset });
    let radius = (rect.width().min(rect.height()) * 0.5 - theme.pad as f32).max(theme.hairline);
    let centre = rect.center();
    painter.circle_stroke(centre, radius, theme.rule_bright_stroke());
    if let Some(frac) = frac {
        // The sweep runs 7 o'clock → 5 o'clock, clockwise.
        let start = std::f32::consts::PI * 0.75;
        let sweep = std::f32::consts::PI * 1.5 * frac.clamp(0.0, 1.0);
        let segments = 24;
        let points: Vec<Pos2> = (0..=segments)
            .map(|i| {
                let a = start + sweep * i as f32 / segments as f32;
                centre + egui::vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        painter.add(egui::Shape::line(points, theme.accent()));
    }
    painter.text(centre, Align2::CENTER_CENTER, value, theme.font_small.clone(), theme.ink);
    painter.text(
        Pos2::new(centre.x, rect.bottom() - theme.hairline),
        Align2::CENTER_BOTTOM,
        label,
        theme.font_small.clone(),
        theme.ink_weak,
    );
    if response.dragged() {
        *acc -= response.drag_delta().y;
    }
    if response.hovered() {
        *acc += ui.input(|i| i.smooth_scroll_delta.y);
    }
    let notch = theme.gear_size.max(1.0);
    let steps = (*acc / notch).trunc();
    *acc -= steps * notch;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, label));
    (response, steps as i32)
}

fn library_key() -> Id {
    Id::new("surface/library-intents")
}

fn loaded_key() -> Id {
    Id::new("surface/library-loaded")
}

/// A control cell asks the library to move, switch columns or load.
pub fn push_library_intent(ctx: &egui::Context, intent: LibraryIntent) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<LibraryIntent>>(library_key()).push(intent));
    ctx.request_repaint();
}

/// The library drains what the grid raised since its last frame.
pub fn take_library_intents(ctx: &egui::Context) -> Vec<LibraryIntent> {
    ctx.data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<LibraryIntent>>(library_key())))
}

/// The library hands the grid the file a `load` picked.
pub fn set_loaded(ctx: &egui::Context, drag: super::FileDrag) {
    ctx.data_mut(|d| d.insert_temp(loaded_key(), Some(drag)));
    ctx.request_repaint();
}

/// The grid takes the file the library loaded, once.
pub fn take_loaded(ctx: &egui::Context) -> Option<super::FileDrag> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Option<super::FileDrag>>(loaded_key()).take())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        // 15 white keys, 10 points each, 100 tall.
        Rect::from_min_max(Pos2::ZERO, Pos2::new(150.0, 100.0))
    }

    #[test]
    fn white_keys_split_the_width_and_black_keys_sit_on_top() {
        let r = rect();
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(5.0, 90.0)), Some(48), "the first white is C");
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(15.0, 90.0)), Some(50), "the second white is D");
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(10.0, 20.0)), Some(49), "C# straddles C|D");
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(145.0, 90.0)), Some(72), "the 25th key is the top C");
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(29.0, 20.0)), Some(52), "no black key between E and F");
        assert_eq!(note_at(r, 48, 0.6, Pos2::new(200.0, 20.0)), None);
    }
}
