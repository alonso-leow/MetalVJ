//! The "new" gallery: one overlay, shaped like the settings window — the
//! sections on the left, the things you can make on the right as a grid of
//! cards, each with a small picture of how it will look. The nodes canvas
//! opens it from its floating `new` button and the sequence from `importar`;
//! both hand it their own [`Section`]s and read back what was [`Outcome::Pick`]ed.
//!
//! A card can be clicked (the caller adds it) or dragged: the drag closes the
//! gallery at once (so the canvas underneath is reachable) and keeps carrying
//! a [`NodeDrag`] until the pointer is released over a drop target;
//! [`paint_drag_ghost`] draws the card under the pointer meanwhile.
//!
//! Nothing here calls the door or knows what a kind is: the caller assembled
//! the cards (regla 5), this file lays them out.

use egui::{Align2, Context, Id, Pos2, Rect, RichText, Sense, Ui, Vec2};

use super::classes::Theme;
use super::components::{header_field, nav_list, panel_frame, paint_icon_stroked, Icon};
use super::i18n::tr;

/// The card grid never has fewer columns than this…
const COLUMNS_MIN: usize = 2;
/// …nor more.
const COLUMNS_MAX: usize = 5;
/// Preview aspect: width over height.
const PREVIEW_ASPECT: f32 = 16.0 / 9.0;
/// The share of the overlay's width the sections column takes; the card grid
/// takes the rest (the 70% the design asks for).
const SECTIONS_SHARE: f32 = 0.3;
/// Ports a preview card draws per side (more would only crowd a thumbnail).
const PREVIEW_PORTS: usize = 4;

/// Who opened the gallery, and so what a pick means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The nodes canvas: a pick adds that node to the open graph.
    Nodes,
    /// The sequence editor's `importar`: a pick adds a clip.
    Sequence,
}

/// A model's picture: its texture, the texture's size and the crop.
pub type Tile = (egui::TextureId, [usize; 2], Rect);

/// The pictures a card shows: one per float variant, drawn as a 2x2 strip.
/// `None` is a variant still pending (a placeholder tile).
pub const STRIP: usize = 4;
pub type Tiles = [Option<Tile>; STRIP];

/// The four cells of a card's 2x2 strip inside `rect`, `gap` apart, in
/// reading order (top-left, top-right, bottom-left, bottom-right).
pub fn strip_cells(rect: Rect, gap: f32) -> [Rect; STRIP] {
    let cw = (rect.width() - gap).max(0.0) / 2.0;
    let ch = (rect.height() - gap).max(0.0) / 2.0;
    let at = |col: usize, row: usize| {
        Rect::from_min_size(
            rect.min + Vec2::new(col as f32 * (cw + gap), row as f32 * (ch + gap)),
            Vec2::new(cw, ch),
        )
    };
    [at(0, 0), at(1, 0), at(0, 1), at(1, 1)]
}

/// One thing the gallery can make: a catalogue kind or a model.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: String,
    pub label: String,
    /// The family heading under the label (`generator`, `effect`…).
    pub family: String,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The card's pictures, one per float variant (`gen.*` models and the
    /// `fx.*` / `mat.*` kinds); empty tiles are placeholders.
    pub tiles: Tiles,
    /// One of the anchor's best fits (the relevance star).
    pub starred: bool,
    /// A global favourite (the star the card's corner shows and toggles).
    pub favourite: bool,
}

/// A left-column division and the cards under it.
pub struct Section {
    pub icon: Icon,
    pub label: String,
    pub items: Vec<Item>,
}

/// What a card carries while it is dragged out of the gallery.
#[derive(Clone, Debug)]
pub struct NodeDrag {
    pub item: Item,
}

/// What the gallery asks its caller to do this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    /// A card was clicked: make this kind.
    Pick(String),
    /// A card is being dragged out; the gallery is closed, the drag lives on.
    Dragging,
    /// Esc or a click outside.
    Close,
}

/// What the gallery remembers between frames.
#[derive(Default)]
pub struct State {
    target: Option<Target>,
    section: usize,
    query: String,
    /// The frame it opened on, so the click that opened it cannot close it.
    opened_at: Option<u64>,
    /// Whether a card was on screen the last time the grid painted: the
    /// caller fetches the cards' pictures only while it is (scroll-aware).
    cards_seen: bool,
    /// The kinds whose cards were on screen the last time the grid painted:
    /// their own previews are fetched, lazily, only for these.
    visible_kinds: Vec<String>,
}

impl State {
    pub fn open(&mut self, target: Target, frame: u64) {
        if self.target != Some(target) {
            self.section = 0;
            self.query.clear();
        }
        self.target = Some(target);
        self.opened_at = Some(frame);
    }

    pub fn close(&mut self) {
        self.target = None;
    }

    pub fn target(&self) -> Option<Target> {
        self.target
    }

    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }

    /// Whether any card was visible on the last painted frame.
    pub fn cards_visible(&self) -> bool {
        self.cards_seen
    }

    /// The kinds whose cards were visible on the last painted frame.
    pub fn visible_kinds(&self) -> &[String] {
        &self.visible_kinds
    }
}

/// How many columns a grid `width` points wide gets: as many cards of
/// [`Theme::gallery_card_min_w`] as fit, between two and five.
pub fn columns_for(theme: &Theme, width: f32) -> usize {
    ((width / theme.gallery_card_min_w).floor() as usize).clamp(COLUMNS_MIN, COLUMNS_MAX)
}

/// Paints the overlay while `state` is open and reports what happened. The
/// caller supplies the sections for [`State::target`] and `title`.
pub fn paint(ctx: &Context, theme: &Theme, state: &mut State, title: &str, sections: &[Section]) -> Outcome {
    if !state.is_open() {
        return Outcome::None;
    }
    let mut outcome = Outcome::None;
    // The frame it opened on: land on the first section that has cards, so
    // an empty leading section (no recent picks yet) does not open blank.
    if state.opened_at == Some(ctx.cumulative_pass_nr()) && state.query.trim().is_empty() {
        let here_empty = sections.get(state.section).map_or(true, |s| s.items.is_empty());
        if here_empty {
            if let Some(i) = sections.iter().position(|s| !s.items.is_empty()) {
                state.section = i;
            }
        }
    }
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let screen = ctx.content_rect();
    let width = (screen.width() * theme.gallery_share).max(theme.region_min.x);
    let height = (screen.height() * theme.gallery_share).max(theme.region_min.y);
    // The same floating window as the "create" button's overlay: its header drags it.
    let shown = floating_window("shell/gallery-overlay", title.to_string())
        .default_pos(screen.center() - Vec2::new(width, height) * 0.5)
        .fixed_size(Vec2::new(width, height))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            panel_frame(theme).show(ui, |ui| {
                let inner_w = ui.available_width();
                let sections_w = (inner_w * SECTIONS_SHARE - theme.item_gap.x).max(theme.control_width);
                let grid_w = (inner_w - sections_w - theme.item_gap.x * 3.0).max(theme.gallery_card_min_w * 2.0);
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(Vec2::new(sections_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(sections_w);
                        section_column(ui, theme, state, sections);
                    });
                    ui.separator();
                    ui.allocate_ui_with_layout(Vec2::new(grid_w, height), egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.set_width(grid_w);
                        if let Some(o) = card_grid(ui, theme, state, sections, height, grid_w) {
                            outcome = o;
                        }
                    });
                });
            });
        });
    let clicked_elsewhere = shown.is_some_and(|w| w.response.clicked_elsewhere());
    if outcome == Outcome::Dragging {
        state.close();
    }
    let fresh = state.opened_at == Some(ctx.cumulative_pass_nr());
    if outcome == Outcome::None && (escape || (!fresh && clicked_elsewhere)) {
        outcome = Outcome::Close;
    }
    if matches!(outcome, Outcome::Close | Outcome::Pick(_)) {
        state.close();
    }
    outcome
}

/// The floating window the "create" button's overlay is (`lanes`): a window
/// that is not collapsible or resizable, and whose header drags it. The
/// gallery and the settings overlay open the same way; each draws its own
/// panel inside.
pub fn floating_window(id: &'static str, title: String) -> egui::Window<'static> {
    egui::Window::new(title).id(Id::new(id)).collapsible(false).resizable(false).frame(egui::Frame::NONE)
}

/// The framed overlay window the "controller button" editor is: header drags,
/// carries the close "x" once `.open(..)` is set. The settings overlay reuses it.
pub fn overlay_window(id: &'static str, title: String) -> egui::Window<'static> {
    egui::Window::new(title).id(Id::new(id)).collapsible(false).resizable(false)
}

/// The left column: a filter and one button per section.
fn section_column(ui: &mut Ui, theme: &Theme, state: &mut State, sections: &[Section]) {
    header_field(ui, theme, &mut state.query, tr("search…"));
    ui.add_space(theme.item_gap.y);
    let rows: Vec<(Icon, &str, Option<String>)> = sections
        .iter()
        .map(|s| (s.icon, s.label.as_str(), Some(s.items.len().to_string())))
        .collect();
    let selected = state.query.trim().is_empty().then_some(state.section);
    if let Some(i) = nav_list(ui, theme, &rows, selected) {
        state.section = i;
        state.query.clear();
    }
}

/// The right column: the section's cards (or every match of the filter), in
/// as many columns as the width allows.
fn card_grid(ui: &mut Ui, theme: &Theme, state: &mut State, sections: &[Section], height: f32, width: f32) -> Option<Outcome> {
    let query = state.query.trim().to_lowercase();
    let items: Vec<&Item> = if query.is_empty() {
        sections.get(state.section).map(|s| s.items.iter().collect()).unwrap_or_default()
    } else {
        sections
            .iter()
            .flat_map(|s| s.items.iter())
            .filter(|i| {
                i.label.to_lowercase().contains(&query)
                    || i.kind.to_lowercase().contains(&query)
                    || super::naming::keywords(&i.kind).iter().any(|k| k.contains(&query))
            })
            .collect()
    };
    if items.is_empty() {
        ui.label(RichText::new(tr("nothing here")).font(theme.font_small.clone()).color(theme.ink_weak));
        return None;
    }
    state.cards_seen = false;
    state.visible_kinds.clear();
    let columns = columns_for(theme, width);
    let gap = theme.item_gap.x;
    let card_w = ((width - gap * (columns as f32 - 1.0) - theme.scroll_gutter) / columns as f32).max(1.0);
    let preview_h = card_w / PREVIEW_ASPECT;
    let label_h = ui.text_style_height(&egui::TextStyle::Body) + ui.text_style_height(&egui::TextStyle::Small);
    let card_h = preview_h + theme.card_margin().sum().y + label_h + theme.item_gap.y;
    let mut outcome = None;
    let mut seen = false;
    egui::ScrollArea::vertical()
        .id_salt("shell/gallery/cards")
        .max_height(height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = theme.item_gap;
            for row in items.chunks(columns) {
                ui.horizontal(|ui| {
                    for item in row {
                        let (o, visible) = card(ui, theme, item, Vec2::new(card_w, card_h));
                        seen |= visible;
                        if visible {
                            state.visible_kinds.push(item.kind.clone());
                        }
                        if let Some(o) = o {
                            outcome = Some(o);
                        }
                    }
                });
            }
        });
    state.cards_seen = seen;
    outcome
}

/// One card: the preview, the label, the family. Click picks; a drag starts
/// the [`NodeDrag`].
/// The second value is whether the card is on screen (the scroll area's
/// clip included), which decides whether its picture is fetched.
fn card(ui: &mut Ui, theme: &Theme, item: &Item, size: Vec2) -> (Option<Outcome>, bool) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let visible = ui.is_rect_visible(rect);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &item.label));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.library_card_corner(), theme.library_card);
    if visible && super::gallery_anim::frames(ui.ctx(), &item.kind).is_some() {
        // Only a card on screen asks for its next frame, and only when it is due.
        let now = ui.ctx().input(|i| i.time);
        super::frames::repaint_after(ui.ctx(), std::time::Duration::from_secs_f64(super::gallery_anim::secs_to_next_frame(now)));
    }
    if response.hovered() {
        // The red emphasis: a wash over the card and an accent outline.
        painter.rect_filled(rect, theme.library_card_corner(), theme.faded(theme.phosphor, theme.hover_alpha));
        painter.rect_stroke(rect, theme.library_card_corner(), theme.accent(), egui::StrokeKind::Inside);
    }
    let pad = theme.card_margin();
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(f32::from(pad.left), f32::from(pad.top)),
        rect.max - Vec2::new(f32::from(pad.right), f32::from(pad.bottom)),
    );
    let preview = Rect::from_min_size(inner.min, Vec2::new(inner.width(), inner.width() / PREVIEW_ASPECT));
    paint_preview(&painter, theme, preview, item);
    // The star corner: a click on it toggles the global favourite, and it is
    // added after the card so it wins the click over the card's own response.
    let star_side = theme.item_gap.x * 2.0 + ui.text_style_height(&egui::TextStyle::Body);
    let star_rect = Rect::from_min_size(rect.right_top() + Vec2::new(-star_side - theme.hairline, theme.hairline), Vec2::splat(star_side));
    let star = ui.interact(star_rect, ui.id().with(("gallery-star", &item.kind)), Sense::click());
    let star_ink = if item.favourite { theme.accent_vivid } else { theme.ink_weak };
    paint_icon_stroked(&painter, star_rect, Icon::Star, theme, theme.line_stroke(star_ink));
    if star.on_hover_text(tr("favourite")).clicked() {
        super::favorites::request_toggle(ui.ctx(), super::favorites::entry_of(item));
        return (None, visible);
    }
    let label = if item.starred { format!("★ {}", item.label) } else { item.label.clone() };
    let fitted = fit_text(&painter, &label, theme.font_body.clone(), theme.ink, inner.width());
    painter.text(Pos2::new(inner.left(), preview.bottom() + theme.item_gap.y), Align2::LEFT_TOP, fitted, theme.font_body.clone(), theme.ink);
    let family = fit_text(&painter, &item.family, theme.font_small.clone(), theme.ink_weak, inner.width());
    painter.text(
        Pos2::new(inner.left(), preview.bottom() + theme.item_gap.y + ui.text_style_height(&egui::TextStyle::Body)),
        Align2::LEFT_TOP,
        family,
        theme.font_small.clone(),
        theme.ink_weak,
    );
    response.dnd_set_drag_payload(NodeDrag { item: item.clone() });
    if response.drag_started() {
        return (Some(Outcome::Dragging), visible);
    }
    (response.clicked().then(|| Outcome::Pick(item.kind.clone())), visible)
}

/// `text` cut to fit `max_w` points with an ellipsis, so a long title never
/// runs under the star corner or past the card's edge.
fn fit_text(painter: &egui::Painter, text: &str, font: egui::FontId, color: egui::Color32, max_w: f32) -> String {
    let fits = |t: &str| painter.layout_no_wrap(t.to_string(), font.clone(), color).size().x <= max_w;
    if fits(text) {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if fits(&candidate) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    chars[..lo].iter().collect::<String>() + "…"
}

/// The thumbnail of a node as the canvas will draw it: the ground, a box with
/// its header and title, its ports on the rim and, for a model, its picture.
pub fn paint_preview(painter: &egui::Painter, theme: &Theme, rect: Rect, item: &Item) {
    // A kind with its GIF shows the animation in place of the schematic box.
    if let Some(frames) = super::gallery_anim::frames(painter.ctx(), &item.kind) {
        let now = painter.ctx().input(|i| i.time);
        let frame = &frames[super::gallery_anim::frame_index(now, frames.len())];
        painter.with_clip_rect(rect).image(frame.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), theme.image_tint);
        return;
    }
    let painter = painter.with_clip_rect(rect);
    painter.rect(rect, theme.corner_radius(), theme.node_canvas, theme.rule_stroke(), egui::StrokeKind::Inside);
    let body = rect.shrink2(Vec2::new(rect.width() * 0.16, rect.height() * 0.12));
    painter.rect(body, theme.corner_radius(), theme.node_body, theme.node_box_stroke(), egui::StrokeKind::Middle);
    let head = Rect::from_min_size(body.min, Vec2::new(body.width(), body.height() * 0.26));
    painter.rect_filled(head, theme.corner_radius(), theme.node_header);
    painter.text(
        head.left_center() + Vec2::new(theme.node_label_inset, 0.0),
        Align2::LEFT_CENTER,
        &item.label,
        theme.font_small.clone(),
        theme.node_title_ink,
    );
    let rest = Rect::from_min_max(Pos2::new(body.left(), head.bottom()), body.max);
    for (cell, tile) in strip_cells(rest.shrink(theme.hairline), theme.hairline).into_iter().zip(&item.tiles) {
        match tile {
            Some((tex, _, uv)) => {
                painter.image(*tex, cell, *uv, theme.image_tint);
            }
            None => {
                painter.rect_filled(cell, 0.0, theme.node_canvas);
                painter.rect_stroke(cell, 0.0, theme.rule_stroke(), egui::StrokeKind::Inside);
            }
        }
    }
    let dots = |names: &[String], x: f32| {
        let n = names.len().min(PREVIEW_PORTS);
        for i in 0..n {
            let y = rest.top() + rest.height() * (i as f32 + 1.0) / (n as f32 + 1.0);
            painter.circle_filled(Pos2::new(x, y), theme.node_port_preview_radius, theme.node_dot_on);
        }
    };
    dots(&item.inputs, body.left());
    dots(&item.outputs, body.right());
}

/// The card under the pointer while one is dragged out of the gallery.
/// Called once a frame by the shell; paints nothing without a payload.
pub fn paint_drag_ghost(ctx: &Context, theme: &Theme) {
    let Some(payload) = egui::DragAndDrop::payload::<NodeDrag>(ctx) else {
        return;
    };
    let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    if !ctx.input(|i| i.pointer.any_down()) {
        return;
    }
    let width = theme.gallery_card_min_w;
    egui::Area::new(Id::new("shell/gallery-drag-ghost"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pointer + Vec2::splat(theme.item_gap.x))
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, width / PREVIEW_ASPECT), Sense::hover());
            paint_preview(ui.painter(), theme, rect, &payload.item);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card's 2x2 strip: four cells, each inside the preview, none overlapping.
    #[test]
    fn the_strip_puts_four_tiles_inside_the_card_preview() {
        let (x, y, w, h, gap) = (10.0, 20.0, 200.0, 120.0, 4.0);
        let rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        let cells = strip_cells(rect, gap);
        assert_eq!(cells.len(), STRIP);
        for c in &cells {
            assert!(c.min.x >= rect.min.x && c.min.y >= rect.min.y, "{c:?} starts inside {rect:?}");
            assert!(c.max.x <= rect.max.x && c.max.y <= rect.max.y, "{c:?} ends inside {rect:?}");
        }
        assert!(cells[0].max.x <= cells[1].min.x, "top row side by side");
        assert!(cells[0].max.y <= cells[2].min.y, "second row below the first");
        assert_eq!(cells[0].min.y, cells[1].min.y);
        assert_eq!(cells[2].min.x, cells[0].min.x);
    }

    /// The overlay and the cards are 40% of their former LINEAR size (the
    /// former share 0.8 and card width 170 on the first theme), so the grid
    /// still gets two columns on a narrow window.
    #[test]
    fn the_gallery_is_forty_percent_of_its_former_size() {
        let theme = Theme::phosphor();
        assert!((theme.gallery_share - 0.8 * 0.4).abs() < 1e-4, "overlay share {}", theme.gallery_share);
        assert!((theme.gallery_card_min_w - 170.0 * 0.4).abs() < 1e-4, "card min width {}", theme.gallery_card_min_w);
        assert_eq!(columns_for(&theme, theme.gallery_card_min_w * 2.0), 2);
    }
}
