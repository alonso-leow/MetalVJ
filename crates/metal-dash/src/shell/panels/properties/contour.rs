//! The one contour editor (`S.F`/`O.1`, 2026-10-05): the widget the 2D mask
//! panel and the 3D wall calibration both draw with.
//!
//! The plan's rule is *"they must be one widget"*: the mask editor and the
//! wall editor may not diverge. This module is that widget — it paints a
//! closed (or in-progress) polygon over a rectangle the host already filled
//! with the device preview, turns pointer and key gestures into a
//! [`ContourEvent`], and **never calls the door**. The host owns posting:
//! `mask.rs` maps `Changed`/`Closed` onto `POST /outputs {op:"mask"}`,
//! `output_device.rs` maps them onto the wall-fit route.
//!
//! Points are in the output's own `0..1` `(x, y)` space, so the same
//! polygon lines up with whatever picture the host paints in `rect` at the
//! device's aspect ratio.

use egui::{Id, Pos2, Rect, Sense, Ui, Vec2};

use crate::shell::classes::Theme;

/// Which gesture the editor is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ContourMode {
    /// A closed polygon: drag its vertices, double-click an edge to insert,
    /// right-click or `Delete` to remove (never below `min_points`).
    Edit,
    /// A free polygon: click appends a vertex, double-click or `Enter`
    /// closes, `Esc` cancels.
    Draw,
}

/// What one frame of the editor produced for its host to post.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ContourEvent {
    /// Nothing happened this frame.
    None,
    /// The whole points array changed (drag/insert/delete). Post it once.
    Changed(Vec<[f32; 2]>),
    /// Draw mode closed on this polygon (at least `min_points` vertices).
    Closed(Vec<[f32; 2]>),
    /// Draw mode cancelled (`Esc`).
    Cancelled,
}

/// One contour editor frame. The host allocates `rect`, paints the device
/// preview into it, then runs this; the widget paints the mask washes,
/// outline and handles on top and returns the gesture.
pub(crate) struct ContourEditor<'a> {
    /// The polygon, in the output's `0..1` space.
    pub points: &'a [[f32; 2]],
    /// Where the polygon is painted (already the device's aspect ratio).
    pub rect: Rect,
    pub mode: ContourMode,
    /// The fewest vertices a closed polygon may keep.
    pub min_points: usize,
    /// Draw mode's in-progress vertices, owned by the host so a polygon
    /// being drawn survives the frame it was started on.
    pub draft: &'a mut Vec<[f32; 2]>,
    /// A fixed-vertex-count editor (wall calibration): drag only, no
    /// edge-insert and no delete.
    pub lock_count: bool,
    pub id: Id,
}

impl ContourEditor<'_> {
    /// Paint and run one frame of the editor.
    pub(crate) fn ui(self, ui: &mut Ui, theme: &Theme) -> ContourEvent {
        let rect = self.rect;
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return ContourEvent::None;
        }
        let to_screen = |p: [f32; 2]| rect.min + Vec2::new(p[0] * rect.width(), p[1] * rect.height());
        let to_norm = |p: Pos2| {
            [
                ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
                ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
            ]
        };
        let handle_r = theme.hairline * theme.handle_hairlines;
        let handle_hit = handle_r * theme.handle_hit;
        let edge_hit = handle_r * theme.edge_hit;
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());

        match self.mode {
            ContourMode::Draw => self.draw_mode(ui, theme, resp, to_screen, to_norm, handle_r),
            ContourMode::Edit => self.edit_mode(ui, theme, rect, resp, to_norm, handle_r, handle_hit, edge_hit),
        }
    }

    // --- draw mode ------------------------------------------------------

    fn draw_mode(
        self,
        ui: &mut Ui,
        theme: &Theme,
        resp: egui::Response,
        to_screen: impl Fn([f32; 2]) -> Pos2,
        to_norm: impl Fn(Pos2) -> [f32; 2],
        handle_r: f32,
    ) -> ContourEvent {
        // `Escape` cancels, `Enter` closes — read before the pointer so a
        // cancel never also appends the click that carried it.
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.draft.clear();
            return ContourEvent::Cancelled;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            return self.close_draft();
        }
        // A double-click is also a click; it closes instead of appending the
        // second vertex at the same spot.
        if resp.double_clicked() {
            return self.close_draft();
        }
        if let (true, Some(p)) = (resp.clicked(), resp.interact_pointer_pos()) {
            self.draft.push(to_norm(p));
        }

        paint_polyline(ui, theme, self.draft, &to_screen, handle_r);
        ContourEvent::None
    }

    fn close_draft(self) -> ContourEvent {
        if self.draft.len() >= self.min_points {
            ContourEvent::Closed(std::mem::take(&mut *self.draft))
        } else {
            ContourEvent::None
        }
    }

    // --- edit mode ------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn edit_mode(
        self,
        ui: &mut Ui,
        theme: &Theme,
        rect: Rect,
        resp: egui::Response,
        to_norm: impl Fn(Pos2) -> [f32; 2],
        handle_r: f32,
        handle_hit: f32,
        edge_hit: f32,
    ) -> ContourEvent {
        let to_screen = |p: [f32; 2]| rect.min + Vec2::new(p[0] * rect.width(), p[1] * rect.height());
        let sel_id = self.id.with("selected");
        let mut selected: Option<usize> = ui.data(|d| d.get_temp(sel_id)).flatten();

        // The washes first, so every handle and the outline sit on top.
        ui.painter().rect_filled(rect, theme.corner_radius(), theme.mask_outside);
        if self.points.len() >= 3 {
            let screen: Vec<Pos2> = self.points.iter().map(|p| to_screen(*p)).collect();
            ui.painter().add(egui::epaint::PathShape {
                points: screen,
                closed: true,
                fill: theme.mask_inside,
                stroke: theme.rule_bright_stroke().into(),
            });
        }

        let mut updated: Vec<[f32; 2]> = self.points.to_vec();
        let mut changed = false;
        let mut over_handle = false;

        for (i, p) in self.points.iter().enumerate() {
            let id = self.id.with(("vertex", i));
            let center = to_screen(*p);
            let hit = Rect::from_center_size(center, Vec2::splat(handle_hit));
            let r = ui.interact(hit, id, Sense::click_and_drag());
            over_handle |= r.hovered();
            let live = if r.dragged() || r.drag_stopped() {
                ui.input(|inp| inp.pointer.interact_pos()).unwrap_or(center)
            } else {
                center
            };
            updated[i] = to_norm(live);
            if r.drag_stopped() {
                changed = true;
            }
            if r.clicked() {
                selected = Some(i);
            }
            if r.secondary_clicked() && !self.lock_count && self.points.len() > self.min_points {
                updated.remove(i);
                selected = None;
                changed = true;
                break;
            }
            let fill = if selected == Some(i) { theme.accent_vivid } else { theme.mask_vertex_handle };
            ui.painter().circle_filled(live, handle_r, fill);
        }

        // `Delete` on the selected vertex — the keyboard twin of the
        // right-click above, same floor.
        if !self.lock_count
            && self.points.len() > self.min_points
            && ui.input(|i| i.key_pressed(egui::Key::Delete))
        {
            if let Some(i) = selected.filter(|i| *i < updated.len()) {
                updated.remove(i);
                selected = None;
                changed = true;
            }
        }

        // A double-click on an edge (never on a handle) inserts the nearest
        // point on that edge.
        if !self.lock_count && !over_handle && resp.double_clicked() {
            if let Some(click) = resp.interact_pointer_pos() {
                if let Some((edge, at)) = nearest_edge(self.points, click, &to_screen, edge_hit) {
                    updated.insert(edge + 1, to_norm(at));
                    changed = true;
                }
            }
        }

        ui.data_mut(|d| d.insert_temp(sel_id, selected));
        if changed {
            ContourEvent::Changed(updated)
        } else {
            ContourEvent::None
        }
    }
}

/// The nearest edge of `points` within `max_dist` of `at`, as
/// `(edge index, projected point)`.
pub(crate) fn nearest_edge(
    points: &[[f32; 2]],
    at: Pos2,
    to_screen: &impl Fn([f32; 2]) -> Pos2,
    max_dist: f32,
) -> Option<(usize, Pos2)> {
    if points.len() < 2 {
        return None;
    }
    let mut best: Option<(usize, Pos2, f32)> = None;
    for i in 0..points.len() {
        let a = to_screen(points[i]);
        let b = to_screen(points[(i + 1) % points.len()]);
        let ab = b - a;
        let len2 = ab.length_sq();
        let t = if len2 <= f32::EPSILON {
            0.0
        } else {
            ((at - a).dot(ab) / len2).clamp(0.0, 1.0)
        };
        let proj = a + ab * t;
        let distance = (at - proj).length();
        if best.map(|(_, _, d)| distance < d).unwrap_or(true) {
            best = Some((i, proj, distance));
        }
    }
    best.filter(|(_, _, d)| *d <= max_dist).map(|(i, p, _)| (i, p))
}

/// The outline plus handles of a polygon the widget is not itself editing
/// (draw mode, and the reused wall corners).
pub(crate) fn paint_polyline(
    ui: &mut Ui,
    theme: &Theme,
    draft: &[[f32; 2]],
    to_screen: &impl Fn([f32; 2]) -> Pos2,
    handle_r: f32,
) {
    for i in 0..draft.len().saturating_sub(1) {
        ui.painter()
            .line_segment([to_screen(draft[i]), to_screen(draft[i + 1])], theme.accent());
    }
    if draft.len() >= 3 {
        ui.painter().line_segment(
            [to_screen(draft[draft.len() - 1]), to_screen(draft[0])],
            theme.rule_bright_stroke(),
        );
    }
    for p in draft {
        ui.painter().circle_filled(to_screen(*p), handle_r, theme.mask_vertex_handle);
    }
}
