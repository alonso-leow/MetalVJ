//! The bottom sheet (after Vaul, vaul.emilkowal.ski) and the forms it carries.
//!
//! - [`sheet`]: a panel that slides up from the bottom edge of an anchor rect
//!   (a sidebar, a region), with a grab handle on top. Esc, a click outside, a
//!   click on the handle or dragging it down closes it. Square, the theme's
//!   panel fill and rule, like every other surface.
//! - [`question_form`]: a question with numbered options and a last "write
//!   your own" row — the agent's `question` tool, a new overlay's kind, any
//!   one-choice form. Digits pick an option while the form has the keyboard.
//! - [`command_list`]: rows of commands (label, hint), filtered by a query.
//!
//! Nothing here calls the door: callers pass the content and read back what
//! was picked (regla 5).

use egui::{Context, Id, Rect, RichText, Sense, Ui, Vec2};

use super::classes::Theme;
use super::components::{header_field, panel_frame, small_text};
use super::i18n::tr;

/// What a [`question_form`] answered this frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// One of the offered options, by index.
    Option(usize),
    /// The operator's own words.
    Own(String),
}

/// Paints the sheet while `open` (or while it slides out) over the bottom of
/// `anchor`. `add` fills it. Closing sets `open` to false.
pub fn sheet(ctx: &Context, theme: &Theme, id: Id, anchor: Rect, open: &mut bool, title: &str, add: impl FnOnce(&mut Ui)) {
    let t = ctx.animate_bool_with_time(id.with("anim"), *open, theme.sheet_anim);
    if t <= 0.0 {
        return;
    }
    let max_h = anchor.height() * theme.sheet_max_share;
    let last_h: f32 = ctx.data(|d| d.get_temp(id.with("h"))).unwrap_or(max_h * 0.5);
    let h = last_h.min(max_h);
    let top = anchor.bottom() - h * ease(t);
    let shown = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor.left(), top))
        .constrain(false)
        .show(ctx, |ui| {
            ui.set_width(anchor.width());
            ui.set_max_width(anchor.width());
            panel_frame(theme).show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The grab handle: a click or a downward drag closes.
                let (row, handle) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme.sheet_handle.y * 3.0), Sense::click_and_drag());
                let bar = Rect::from_center_size(row.center(), theme.sheet_handle);
                ui.painter().rect_filled(bar, theme.square_corner(), if handle.hovered() { theme.ink } else { theme.ink_weak });
                handle.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("close")));
                if handle.clicked() || (handle.drag_stopped() && handle.drag_delta().y >= 0.0 && ctx.input(|i| i.pointer.velocity().y) >= 0.0) {
                    *open = false;
                }
                if !title.is_empty() {
                    ui.label(RichText::new(title).font(theme.font_heading.clone()).color(theme.ink_weak));
                    ui.add_space(theme.item_gap.y);
                }
                egui::ScrollArea::vertical()
                    .id_salt(id.with("scroll"))
                    .max_height(max_h - theme.sheet_handle.y * 3.0)
                    .auto_shrink([false, true])
                    .show(ui, add);
            });
        });
    let rect = shown.response.rect;
    ctx.data_mut(|d| d.insert_temp(id.with("h"), rect.height()));
    let opened_now = ctx.data(|d| d.get_temp::<bool>(id.with("was"))) != Some(true);
    ctx.data_mut(|d| d.insert_temp(id.with("was"), *open));
    if *open && !opened_now {
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        // A press above the sheet on the panel underneath closes it; a press on
        // a popup or menu the sheet opened (another foreground layer) does not.
        let outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| {
                !rect.contains(p)
                    && anchor.contains(p)
                    && p.y < rect.top()
                    && ctx.layer_id_at(p).is_none_or(|l| l.order < egui::Order::Foreground)
            });
        if escape || outside {
            *open = false;
        }
    }
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// A question, its options as numbered square rows, and the "write your own"
/// row last. `own` keeps the typed answer between frames.
pub fn question_form(ui: &mut Ui, theme: &Theme, id: Id, question: &str, options: &[String], own: &mut String) -> Option<Answer> {
    let mut out = None;
    if !question.is_empty() {
        ui.label(RichText::new(question).font(theme.font_body.clone()).color(theme.ink));
        ui.add_space(theme.item_gap.y);
    }
    let field_id = id.with("own");
    let typing = ui.memory(|m| m.has_focus(field_id));
    for (i, label) in options.iter().enumerate() {
        let key = format!("{}", i + 1);
        if option_row(ui, theme, &key, label, false).clicked() {
            out = Some(Answer::Option(i));
        }
        if !typing && i < 9 {
            let k = digit_key(i + 1);
            if ui.input_mut(|inp| inp.consume_key(egui::Modifiers::NONE, k)) {
                out = Some(Answer::Option(i));
            }
        }
    }
    // The last row: the operator's own answer.
    ui.horizontal(|ui| {
        ui.label(small_text(theme, format!("{}", options.len() + 1)).color(theme.ink_weak));
        let w = ui.available_width() - ui.spacing().interact_size.x - ui.spacing().item_spacing.x;
        let field = ui.add(
            egui::TextEdit::singleline(own)
                .id(field_id)
                .desired_width(w.max(theme.text_input_width))
                .margin(theme.field_pad)
                .hint_text(tr("write your own answer…")),
        );
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let send = ui.add_enabled(!own.trim().is_empty(), egui::Button::new(small_text(theme, "↑")).corner_radius(theme.square_corner()));
        send.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tr("send")));
        if (enter || send.clicked()) && !own.trim().is_empty() {
            out = Some(Answer::Own(std::mem::take(own).trim().to_string()));
        }
    });
    out
}

fn digit_key(n: usize) -> egui::Key {
    use egui::Key::*;
    [Num1, Num2, Num3, Num4, Num5, Num6, Num7, Num8, Num9][n - 1]
}

/// One full-width square row: a key (number or glyph) at the left, the label,
/// the whole row clickable.
pub fn option_row(ui: &mut Ui, theme: &Theme, key: &str, label: &str, selected: bool) -> egui::Response {
    let pad = ui.spacing().button_padding;
    let galley = ui.painter().layout(label.to_string(), theme.font_small.clone(), theme.ink, ui.available_width() - pad.x * 4.0 - theme.font_small.size * 2.0);
    let size = Vec2::new(ui.available_width(), galley.size().y.max(ui.spacing().interact_size.y) + pad.y * 2.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, label));
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            theme.faded(theme.phosphor, 48)
        } else if response.hovered() {
            theme.library_card_hover
        } else {
            theme.inset
        };
        ui.painter().rect(rect, theme.square_corner(), fill, theme.rule_stroke(), egui::StrokeKind::Inside);
        let key_w = theme.font_small.size * 2.0;
        ui.painter().text(
            egui::pos2(rect.left() + pad.x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            key,
            theme.font_small.clone(),
            theme.ink_weak,
        );
        ui.painter().galley(egui::pos2(rect.left() + pad.x * 2.0 + key_w, rect.center().y - galley.size().y / 2.0), galley, theme.ink);
    }
    response
}

/// One command: what it says, a hint, and the key the caller acts on.
#[derive(Debug, Clone)]
pub struct Command {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub selected: bool,
}

/// A filter field over grouped commands; returns the picked command's key.
pub fn command_list(ui: &mut Ui, theme: &Theme, query: &mut String, groups: &[(String, Vec<Command>)]) -> Option<String> {
    header_field(ui, theme, query, tr("search…"));
    ui.add_space(theme.item_gap.y);
    let q = query.trim().to_lowercase();
    let mut picked = None;
    for (title, cmds) in groups {
        let shown: Vec<&Command> = cmds.iter().filter(|c| q.is_empty() || c.label.to_lowercase().contains(&q) || c.hint.to_lowercase().contains(&q)).collect();
        if shown.is_empty() {
            continue;
        }
        super::components::subtitle(ui, theme, title);
        for c in shown {
            let mark = if c.selected { "●" } else { "›" };
            let label = if c.hint.is_empty() { c.label.clone() } else { format!("{} — {}", c.label, c.hint) };
            if option_row(ui, theme, mark, &label, c.selected).clicked() {
                picked = Some(c.key.clone());
            }
        }
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ease_lands_on_both_ends() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!(ease(0.5) > 0.5, "fast out, slow in");
    }

    #[test]
    fn digits_map_to_their_keys() {
        assert_eq!(digit_key(1), egui::Key::Num1);
        assert_eq!(digit_key(9), egui::Key::Num9);
    }
}
