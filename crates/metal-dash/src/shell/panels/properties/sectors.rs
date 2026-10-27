//! `venue.surface`'s `grid` folder: the sector editor (plan "# The room").
//!
//! A sector grid splits **one output** into the pieces each input slot of the
//! surface paints — "a grid of each pixel output". The operator draws the
//! split lines on the output's own frame (the same `/output.jpg` picture the
//! mask and pins editors paint); a line is a *streak*, two points (start and
//! end) in the device's `0..1` space. With `magnet` on, both ends snap to the
//! nearest 1/8, so a streak always starts **and** ends on a raster line.
//!
//! **Reads and writes are JSON, never a Rust type.** The kernel behind the
//! grid (`metal_perform::venue::SectorGrid`, a sibling's) is read off the
//! venue detail — `GET /venue?id=` → `venue.surfaces[i].sectors` — and the
//! split polygons off `sector_splits`, both defensively: `source` absent →
//! `"projected"`, `ratio` absent → `1.0`, `points` absent → empty, `magnet`
//! absent → off. Every edit posts the whole grid in one
//! `POST /venue { op: "surface.sectors" }` — the same "post the whole array"
//! shape `contour`/`mask` use — with the stroke's `stroke_width` and
//! `stroke_color` in the same body (the route accepts both alongside the
//! grid, so a grid and its look are one write).
//!
//! The stroke rows mirror `mask.rs` exactly — `MASK_STROKE_MAX`, the same
//! width/colour/alpha rows — because the grid's lines are the mask editor's
//! bands, on a different surface.
//!
//! The half-drawn streak (a start placed, waiting for its end) and the point
//! being dragged live in egui temp memory keyed by `(venue_id, surface_id)`,
//! the way `mask.rs` keeps `mask_draft`, so a click that starts a line
//! survives the frame.

use crate::shell::components::small_text;
use egui::{Id, Pos2, Rect, Sense, Ui, Vec2};
use crate::shell::components::{handle_dot, handle_ring};
use metal_notation::color::Hsb;
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{colour_row, dropdown, kv_row, kv_wrap, label_row, note, num_value};
use crate::shell::i18n::{tr, trf};

use super::mask::MASK_STROKE_MAX;

/// The magnet's step: "the nearest 1/8", the raster a click snaps to.
const MAGNET_STEP: f32 = 1.0 / 8.0;
/// A split band's wash, as a fraction of the theme's contour wash — a tint,
/// so the device frame still reads through the grid.
const BAND_WASH: f32 = 0.55;
/// The frame's aspect until the device answers: 16:9.
const DEFAULT_ASPECT: f32 = 16.0 / 9.0;
/// The `ratio` the kernel accepts, and the step the row walks it by.
const RATIO_RANGE: std::ops::RangeInclusive<f32> = 0.2..=8.0;
const RATIO_STEP: f32 = 0.1;

/// The decimal places a step of `step` needs in its readout (`0.1` → 1).
fn decimals_of(step: f32) -> usize {
    let mut n = 0;
    let mut scaled = step;
    while n < 6 && (scaled - scaled.round()).abs() > f32::EPSILON {
        scaled *= 10.0;
        n += 1;
    }
    n
}

/// The grid read off one surface, with the kernel's defaults for every
/// absent field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Grid {
    pub source: String,
    pub ratio: f32,
    pub points: Vec<[f32; 2]>,
    pub magnet: bool,
    pub stroke_width: f32,
    pub stroke_color: [f32; 4],
}

impl Grid {
    /// `surface`'s `sectors` (an absent or null one is the fresh default).
    pub(crate) fn of(surface: &Value) -> Grid {
        let sectors = surface.get("sectors").filter(|s| !s.is_null());
        let points = sectors
            .and_then(|s| s["points"].as_array())
            .map(|raw| raw.iter().filter_map(read_point).collect())
            .unwrap_or_default();
        let stroke_color = sectors
            .and_then(|s| s["stroke_color"].as_array())
            .map(|c| {
                let ch = |i: usize| c.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
                [ch(0), ch(1), ch(2), ch(3)]
            })
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        Grid {
            source: sectors
                .and_then(|s| s["source"].as_str())
                .unwrap_or("projected")
                .to_string(),
            ratio: sectors.and_then(|s| s["ratio"].as_f64()).unwrap_or(1.0) as f32,
            points,
            magnet: sectors.and_then(|s| s["magnet"].as_bool()).unwrap_or(false),
            stroke_width: sectors.and_then(|s| s["stroke_width"].as_f64()).unwrap_or(0.0) as f32,
            stroke_color,
        }
    }

    /// How many sectors the grid makes: one per input slot, so each streak
    /// (two ends) splits off one more piece.
    pub(crate) fn count(&self) -> usize {
        self.points.len() / 2 + 1
    }
}

fn read_point(p: &Value) -> Option<[f32; 2]> {
    Some([p.get(0)?.as_f64()? as f32, p.get(1)?.as_f64()? as f32])
}

/// `p` snapped to the nearest [`MAGNET_STEP`] on both axes.
pub(crate) fn snap(p: [f32; 2]) -> [f32; 2] {
    let axis = |v: f32| ((v / MAGNET_STEP).round() * MAGNET_STEP).clamp(0.0, 1.0);
    [axis(p[0]), axis(p[1])]
}

fn drag_key(venue: &str, surface: &str) -> Id {
    Id::new(("shell/region/properties/sectors-drag", venue, surface))
}

fn note_key() -> Id {
    Id::new("shell/region/properties/sectors-note")
}

/// The operator's grid in progress for `(venue, surface)`, in egui temp
/// memory the way `mask.rs` keeps `mask_draft`: a point placed this frame is
/// there next frame, whether or not the route's reply echoes it yet.
fn draft_key(venue: &str, surface: &str) -> Id {
    Id::new(("shell/region/properties/sectors-draft", venue, surface))
}

/// The draft's stored shape — every field of a [`Grid`], flat, so the temp
/// store never carries the widget's own type across builds.
type Draft = (String, f32, Vec<[f32; 2]>, bool, f32, [f32; 4]);

fn draft(ui: &Ui, venue: &str, surface: &str) -> Option<Grid> {
    let (source, ratio, points, magnet, stroke_width, stroke_color) =
        ui.data(|d| d.get_temp::<Draft>(draft_key(venue, surface)))?;
    Some(Grid { source, ratio, points, magnet, stroke_width, stroke_color })
}

fn remember(ui: &Ui, venue: &str, surface: &str, grid: &Grid) {
    let value: Draft = (
        grid.source.clone(),
        grid.ratio,
        grid.points.clone(),
        grid.magnet,
        grid.stroke_width,
        grid.stroke_color,
    );
    ui.data_mut(|d| d.insert_temp(draft_key(venue, surface), value));
}

/// Paint the `venue.surface` node's `grid` folder. `surface` is the row of
/// the venue detail (`GET /venue?id=`) the node names, or `Null` when it
/// names none.
#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    venue_id: &str,
    surface: &Value,
    state: &mut super::PanelState,
) {
    crate::shell::components::section_heading(ui, theme, tr("grid"));
    // The room and the surface are the caller's: the node names them, and the
    // graph's own room fills in when it does not. `node` is here so the
    // signature matches every sibling section and a later read can use it.
    let _ = node;
    let surface_id = surface["id"].as_str().filter(|s| !s.is_empty()).unwrap_or_default().to_string();
    let detail = super::surface::get_venue(door, venue_id);
    let splits = surfaces_splits(detail.as_ref(), &surface_id);
    // The grid the venue detail already carries — the persisted truth. The
    // operator's half-drawn grid lives beside it in temp memory: a click is
    // posted *and* kept, so a route that echoes the old grid (or a test
    // door that answers `{}`) never loses the point the operator just placed.
    let saved = Grid::of(surface);
    let mut grid = draft(ui, venue_id, &surface_id).unwrap_or_else(|| saved.clone());
    if grid != saved {
        remember(ui, venue_id, &surface_id, &grid);
    }

    // --- source ----------------------------------------------------------
    let mut source = grid.source.clone();
    if kv_row(ui, theme, tr("source"), |ui| {
        dropdown(
            ui,
            theme,
            ("shell/region/properties/sectors-source", venue_id, surface_id.as_str()),
            &mut source,
            &[
                ("projected".to_string(), tr("projected")),
                ("points".to_string(), tr("points")),
            ],
        )
    })
    .changed()
    {
        grid.source = source;
        post(door, ui, venue_id, &surface_id, &grid);
    }

    // --- ratio, then the count derived from the grid ---------------------
    let mut ratio = grid.ratio;
    kv_row(ui, theme, tr("ratio"), |ui| {
        let width = ui.available_width();
        let decimals = decimals_of(RATIO_STEP);
        num_value(ui, theme, width, &mut ratio, RATIO_RANGE, "", decimals, 1.0, true)
    });
    if (ratio - grid.ratio).abs() > f32::EPSILON && RATIO_RANGE.contains(&ratio) {
        grid.ratio = ratio;
        post(door, ui, venue_id, &surface_id, &grid);
    }
    let count = if splits.is_empty() { grid.count() } else { splits.len() };
    label_row(ui, theme, tr("sectors"), |ui| {
        ui.label(small_text(theme, count.to_string()).color(theme.ink))
    });
    // The canvas's own scroll anchor: the frame's top-left label, painted
    // before the canvas so a scroll really brings the whole editor up. It is
    // the same string the section heading carries, which the fold already
    // shows — a quiet duplicate, and the one handle a kittest can grab.
    ui.add(egui::Label::new(small_text(theme, tr("output frame")).color(theme.ink_weak)));

    // --- magnet ----------------------------------------------------------
    let mut magnet = grid.magnet;
    if ui
        .checkbox(&mut magnet, small_text(theme, tr("magnet")))
        .changed()
    {
        grid.magnet = magnet;
        post(door, ui, venue_id, &surface_id, &grid);
    }

    // --- the canvas ------------------------------------------------------
    let graph = super::shared::get(door, "GET", "/graph", &[("id", gid.to_string())]).unwrap_or(Value::Null);
    let aspect = read_aspect(door, gid, &graph, node_id);
    if let Some(points) =
        canvas(ui, theme, door, gid, node_id, &graph, venue_id, &surface_id, &grid, &splits, aspect, state)
    {
        grid.points = points;
        post(door, ui, venue_id, &surface_id, &grid);
    }

    // --- the stroke, same tokens and ranges as `mask.rs` -----------------
    // Only a hand-drawn grid carries a look: a projected grid's split is the
    // layout's, so the rows read as inactive.
    ui.add_enabled_ui(grid.source == "points", |ui| {
        let mut changed = false;
        let mut width = grid.stroke_width;
        kv_row(ui, theme, tr("stroke"), |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut width, 0.0..=MASK_STROKE_MAX, "", 0, 0.0, true);
            changed |= r.changed();
            r
        });
        grid.stroke_width = width;
        let [r, g, b, mut alpha] = grid.stroke_color;
        let mut hsb = Hsb::from_rgb(r, g, b);
        if colour_row(ui, theme, tr("colour"), &mut hsb).changed() {
            let [r, g, b] = hsb.to_rgb();
            grid.stroke_color = [r, g, b, alpha];
            changed = true;
        }
        kv_row(ui, theme, tr("alpha"), |ui| {
            let w = ui.available_width();
            let r = num_value(ui, theme, w, &mut alpha, 0.0..=1.0, "", 2, 1.0, true);
            changed |= r.changed();
            r
        });
        grid.stroke_color[3] = alpha;
        if changed {
            post(door, ui, venue_id, &surface_id, &grid);
        }
    });

    // --- `+ línea` / `close` ---------------------------------------------
    let mut cleared = false;
    kv_wrap(ui, theme, "", |ui| {
        if ui.button(small_text(theme, tr("+ línea"))).clicked() {
            // A new streak: the next click starts one, whatever was grabbed.
            ui.data_mut(|d| d.insert_temp(drag_key(venue_id, &surface_id), None::<usize>));
        }
        if ui
            .button(small_text(theme, tr("clear grid")))
            .clicked()
        {
            ui.data_mut(|d| d.insert_temp(drag_key(venue_id, &surface_id), None::<usize>));
            // Clearing the points is the draft's clear: an empty `Grid`.
            ui.data_mut(|d| d.remove_temp::<Draft>(draft_key(venue_id, &surface_id)));
            cleared = true;
        }
    });
    if cleared {
        let clear = Grid { points: Vec::new(), ..grid.clone() };
        post(door, ui, venue_id, &surface_id, &clear);
    }

    remember(ui, venue_id, &surface_id, &grid);

    if let Some(msg) = ui.data(|d| d.get_temp::<String>(note_key())) {
        note(ui, theme, &msg);
    }
    if surface_id.is_empty() {
        note(ui, theme, tr("— pick a surface on the surface step first —"));
    } else if grid.points.is_empty() && splits.is_empty() {
        note(ui, theme, tr("click on the frame to draw a split line"));
    }
}

/// `POST /venue { op: "surface.sectors", … }` — one write of the whole grid,
/// the stroke included. The route's refusal is kept under the editor rather
/// than lost.
fn post(door: &dyn Call, ui: &mut Ui, venue: &str, surface: &str, grid: &Grid) {
    if surface.is_empty() {
        return;
    }
    let body = json!({
        "op": "surface.sectors",
        "venue": venue,
        "surface": surface,
        "source": grid.source,
        "ratio": grid.ratio,
        "points": grid.points,
        "magnet": grid.magnet,
        "stroke_width": grid.stroke_width,
        "stroke_color": grid.stroke_color,
    });
    let note = match super::shared::post_reply(door, "/venue", body) {
        Ok(_) => Value::Null,
        Err((status, msg)) => {
            Value::String(trf("grid write failed: {status} {msg}", &[("status", &status), ("msg", &msg)]))
        }
    };
    ui.data_mut(|d| d.insert_temp(note_key(), note));
}

/// `sector_splits` of the venue detail for `surface` — one polygon per input
/// slot, in the device's `0..1` space. The kernel files them on the surface;
/// a reply that carries them top-level is read too.
fn surfaces_splits(detail: Option<&Value>, surface: &str) -> Vec<Vec<[f32; 2]>> {
    let Some(detail) = detail else { return Vec::new() };
    let on_surface = detail["venue"]["surfaces"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| !surface.is_empty() && s["id"] == json!(surface))
        .and_then(|s| s["sector_splits"].as_array());
    let raw = on_surface.or_else(|| detail["sector_splits"].as_array());
    raw.into_iter()
        .flatten()
        .filter_map(|poly| {
            let pts: Vec<[f32; 2]> = poly.as_array()?.iter().filter_map(read_point).collect();
            (pts.len() >= 2).then_some(pts)
        })
        .collect()
}

/// The aspect of the output this node drives, off `GET /state`'s own sizes;
/// 16:9 until the graph reaches one (the editor still works, it just paints
/// the default frame).
fn read_aspect(door: &dyn Call, gid: &str, graph: &Value, node_id: &str) -> f32 {
    let Some(device) = crate::shell::collection::device_of(gid, graph, Some(node_id)) else {
        return DEFAULT_ASPECT;
    };
    super::surface::get_outputs(door)
        .iter()
        .find(|o| o["id"] == json!(device))
        .map(|o| {
            let w = o["width"].as_f64().unwrap_or(1920.0) as f32;
            let h = o["height"].as_f64().unwrap_or(1080.0) as f32;
            (w.max(1.0) / h.max(1.0)).clamp(0.2, 5.0)
        })
        .unwrap_or(DEFAULT_ASPECT)
}

/// The editor over the output frame. Returns the whole points array when a
/// click, a drag or a delete changed it this frame.
#[allow(clippy::too_many_arguments)]
fn canvas(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    graph: &Value,
    venue: &str,
    surface: &str,
    grid: &Grid,
    splits: &[Vec<[f32; 2]>],
    aspect: f32,
    state: &mut super::PanelState,
) -> Option<Vec<[f32; 2]>> {
    if surface.is_empty() {
        return None;
    }
    let width = theme.mask_canvas_width.min(ui.available_width());
    let size = Vec2::new(width, width / aspect.max(0.2));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    state.sector_rects.insert((venue.to_string(), surface.to_string()), rect);

    // The output's own frame under the grid, from `/output.jpg?device=`
    // exactly as `mask_canvas` asks for it.
    let device = crate::shell::collection::device_of(gid, graph, Some(node_id));
    paint_frame(ui, theme, door, device.as_deref(), rect, width);

    // The kernel's polygons, or — before it answers — one band per streak.
    if splits.is_empty() {
        for (i, line) in grid.points.chunks_exact(2).enumerate() {
            let screen: Vec<Pos2> = line.iter().map(|p| to_screen(rect, *p)).collect();
            paint_band(ui, theme, &screen, i + 1);
        }
    } else {
        for (i, poly) in splits.iter().enumerate() {
            let screen: Vec<Pos2> = poly.iter().map(|p| to_screen(rect, *p)).collect();
            paint_band(ui, theme, &screen, i + 1);
        }
    }

    let mut points = grid.points.clone();
    let gkey = drag_key(venue, surface);
    let mut dragging: Option<usize> = ui.data(|d| d.get_temp(gkey)).unwrap_or(None);
    let hit_box = theme.hairline * theme.handle_hairlines * theme.handle_hit;
    let hit = |points: &[[f32; 2]], p: Pos2| -> Option<usize> {
        points.iter().position(|q| to_screen(rect, *q).distance(p) <= hit_box)
    };
    let place = |p: Pos2, magnet: bool| {
        let q = to_norm(rect, p);
        if magnet { snap(q) } else { q }
    };

    let mut changed = false;
    let pointer = resp.interact_pointer_pos();

    // A press on a point grabs it; the drag moves it (snapped); the release
    // settles it. A press on empty frame starts a streak instead.
    if resp.drag_started() {
        dragging = pointer.and_then(|p| hit(&points, p));
    }
    if resp.dragged() {
        if let (Some(i), Some(p)) = (dragging, pointer) {
            let q = place(p, grid.magnet);
            if points.get(i).copied() != Some(q) {
                points[i] = q;
                changed = true;
            }
        }
    }
    if resp.drag_stopped() {
        dragging = None;
    }

    // Right-click removes the point under it.
    if resp.secondary_clicked() {
        if let Some(i) = pointer.and_then(|p| hit(&points, p)) {
            points.remove(i);
            dragging = None;
            changed = true;
        }
    }

    // A plain click on empty frame appends one end of the streak. The
    // magnet has already snapped it, so it starts on a raster line.
    if resp.clicked() && !resp.secondary_clicked() && dragging.is_none() {
        if let Some(p) = pointer {
            if hit(&points, p).is_none() {
                points.push(place(p, grid.magnet));
                changed = true;
            }
        }
    }

    ui.data_mut(|d| d.insert_temp(gkey, dragging));

    for (i, p) in points.iter().enumerate() {
        let live = dragging == Some(i);
        let fill = if live { theme.accent_vivid } else { theme.mask_vertex_handle };
        handle_dot(ui.painter(), theme, to_screen(rect, *p), fill);
    }
    // The point just placed, waiting for its pair: a half-streak cue.
    if let Some(last) = points.last().filter(|_| points.len() % 2 == 1) {
        let at = to_screen(rect, *last);
        handle_ring(ui.painter(), theme, at);
    }

    changed.then_some(points)
}

/// The output frame under the grid, from `/output.jpg?device=` like
/// `mask_canvas` — the same pump, so two editors share one fetch.
fn paint_frame(ui: &mut Ui, theme: &Theme, door: &dyn Call, device: Option<&str>, rect: Rect, width: f32) {
    let pic = device.and_then(|device| preview::picture(ui, door, device, width));
    match pic {
        Some((id, _)) => {
            let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            ui.painter().image(id, rect, full, theme.image_tint);
        }
        None => {
            ui.painter().rect_filled(rect, theme.corner_radius(), theme.tube);
            if let Some(device) = device {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    device,
                    theme.font_small.clone(),
                    theme.ink_weak,
                );
            }
        }
    }
}

/// A split band: the theme's contour wash, a bright rule outline and the
/// slot's number on a small plate at its centre.
fn paint_band(ui: &mut Ui, theme: &Theme, screen: &[Pos2], n: usize) {
    if screen.len() < 2 {
        return;
    }
    if screen.len() >= 3 {
        ui.painter().add(egui::epaint::PathShape {
            points: screen.to_vec(),
            closed: true,
            fill: theme.mask_inside.linear_multiply(BAND_WASH),
            stroke: theme.rule_bright_stroke().into(),
        });
    } else {
        ui.painter().line_segment([screen[0], screen[1]], theme.accent());
    }
    let centroid = screen.iter().fold(Vec2::ZERO, |a, p| a + p.to_vec2()) / screen.len() as f32;
    let galley = ui.painter().layout_no_wrap(n.to_string(), theme.font_small.clone(), theme.accent_vivid);
    let plate = Rect::from_center_size(centroid.to_pos2(), galley.size()).expand(theme.hairline);
    ui.painter().rect_filled(plate, theme.corner_radius(), theme.mask_outside);
    ui.painter().galley(plate.center() - galley.size() / 2.0, galley, theme.accent_vivid);
}

fn to_screen(rect: Rect, p: [f32; 2]) -> Pos2 {
    rect.min + Vec2::new(p[0] * rect.width(), p[1] * rect.height())
}

fn to_norm(rect: Rect, p: Pos2) -> [f32; 2] {
    [
        ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0),
        ((p.y - rect.min.y) / rect.height()).clamp(0.0, 1.0),
    ]
}

/// The canvas's preview: the one [`crate::shell::frames::Frame`] this module
/// owns, filed by device, so the editor gets the mask canvas's moving picture
/// without a `PanelState` field. `Frame` is moved in and out of egui temp
/// memory each frame — it holds its worker's receiver, so it cannot be
/// cloned — which is the same trade a field would make, one `data_mut` later.
mod preview {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::call::Call;
    use crate::shell::frames::Frame;

    thread_local! {
        // One `Frame` per device. The frame holds its worker's receiver (not
        // `Send`/`Sync`, not `Clone`), so it cannot live in egui's
        // `Send + Sync` temp store — but it can live here, `Rc`-shared, and
        // be ticked in place each frame.
        static FRAMES: RefCell<Vec<(String, Rc<RefCell<Frame>>)>> = const {
            RefCell::new(Vec::new())
        };
    }

    fn with_frame<T>(device: &str, f: impl FnOnce(&mut Frame) -> T) -> T {
        FRAMES.with(|frames| {
            let rc = {
                let mut frames = frames.borrow_mut();
                match frames.iter().find(|(d, _)| d == device) {
                    Some((_, rc)) => Rc::clone(rc),
                    None => {
                        let rc = Rc::new(RefCell::new(Frame::default()));
                        frames.push((device.to_string(), Rc::clone(&rc)));
                        rc
                    }
                }
            };
            let mut frame = rc.borrow_mut();
            f(&mut frame)
        })
    }

    pub(super) fn picture(
        ui: &mut egui::Ui,
        door: &dyn Call,
        device: &str,
        width: f32,
    ) -> Option<(egui::TextureId, [usize; 2])> {
        let query = vec![
            ("device", device.to_string()),
            ("w", (width.round() as u32).to_string()),
        ];
        with_frame(device, |frame| {
            frame.picture(ui.ctx(), door, "/output.jpg", &query, crate::shell::frames::live_gap())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An absent or null `sectors` is the kernel's own default, never a panic.
    #[test]
    fn a_surface_with_no_sectors_reads_the_defaults() {
        let empty = Grid::of(&json!({ "id": "front" }));
        assert_eq!(empty.source, "projected");
        assert_eq!(empty.ratio, 1.0);
        assert!(empty.points.is_empty());
        assert!(!empty.magnet);
        assert_eq!(empty.count(), 1, "no streak, one sector");
        assert_eq!(Grid::of(&json!({ "id": "front", "sectors": null })), empty);
    }

    #[test]
    fn a_saved_grid_round_trips() {
        let grid = Grid::of(&json!({ "sectors": {
            "source": "points", "ratio": 2.0,
            "points": [[0.25, 0.5], [0.75, 0.5]],
            "magnet": true, "stroke_width": 3.0, "stroke_color": [1.0, 0.0, 0.0, 0.5],
        }}));
        assert_eq!(grid.source, "points");
        assert_eq!(grid.ratio, 2.0);
        assert_eq!(grid.points, vec![[0.25, 0.5], [0.75, 0.5]]);
        assert!(grid.magnet);
        assert_eq!(grid.stroke_width, 3.0);
        assert_eq!(grid.stroke_color, [1.0, 0.0, 0.0, 0.5]);
        assert_eq!(grid.count(), 2, "one streak splits the output in two");
    }

    #[test]
    fn the_magnet_snaps_both_axes_to_the_eighth() {
        assert_eq!(snap([0.31, 0.77]), [0.25, 0.75]);
        assert_eq!(snap([0.99, -1.0]), [1.0, 0.0]);
        for v in [0.0, 0.125, 0.7, 1.0] {
            let [x, y] = snap([v, v]);
            assert!(((x * 8.0).round() - x * 8.0).abs() < 1e-5);
            assert_eq!(x, y);
        }
    }
}
