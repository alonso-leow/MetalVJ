//! The Stage hitbox view (`metal_stage`): the grid of points the fixture AI
//! speaks in, with the venue's own fixtures placed in their cells.
//!
//! Three levels, each a column of cells: the floor and the rig are 2 × 3
//! (`back`, `pro`, `house`), the zenith 3 × 3 (`back`, `mid`, `pro`). A cell
//! holds the fixtures the venue's stage placements put there, with their count;
//! a chip is one fixture (its type letter, `*` when its aim was hand-tuned, its
//! id on hover). Dragging a chip onto another cell pins it there through
//! `POST /venue { op: "fixture", stage_point }`, the same fixture route the
//! Settings page posts.
//!
//! The view reads the `stage` block of `GET /venue?id=` (`call::venue::stage_reply`,
//! which asks `metal_perform::stage::patch_of`): the panel never re-derives a
//! placement from the venue file (rule 9).
//!
//! It is shown in two places, one body: the `hitbox` folder of a `venue.fixtures`
//! node's properties page ([`section`]) and the venue canvas's `stage hitbox`
//! toggle ([`super::venue_canvas`]).

use std::collections::BTreeMap;

use egui::{Align2, Id, Pos2, Rect, RichText, Sense, Ui, UiBuilder};
use metal_stage::{Col, Level, Point, Row};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::note;
use crate::shell::i18n::{tr, trf};

use super::venue_map;

/// The rows a level may have, front to back as the cells read top to bottom.
const ROWS: [Row; 4] = [Row::Back, Row::Mid, Row::Pro, Row::House];
/// The columns, left to right as seen from the audience.
const COLS: [Col; 3] = [Col::L, Col::C, Col::R];

/// One fixture of the stage: what its chip shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chip {
    pub fixture: String,
    /// The venue's fixture word (`wash`, `moving`, …).
    pub kind: String,
    /// The aim was hand-tuned (the venue pose wins over the point's default).
    pub tuned: bool,
    /// The fixture is pinned to this point (`stage_point`), not the nearest one.
    pub pinned: bool,
}

/// The fixtures of one stage, per grid point, in the venue's own order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Board {
    pub cells: BTreeMap<Point, Vec<Chip>>,
}

impl Board {
    /// The board of a `GET /venue` reply's `stage` block. A placement whose
    /// point word does not parse is skipped rather than painted in a guess.
    pub(crate) fn from_stage(stage: &Value) -> Board {
        let mut cells: BTreeMap<Point, Vec<Chip>> = BTreeMap::new();
        for p in stage["placements"].as_array().into_iter().flatten() {
            let Some(point) = p["point"].as_str().and_then(Point::parse) else {
                continue;
            };
            cells.entry(point).or_default().push(Chip {
                fixture: p["fixture"].as_str().unwrap_or_default().to_string(),
                kind: p["kind"].as_str().unwrap_or_default().to_string(),
                tuned: p["tuned"].as_bool().unwrap_or(false),
                pinned: p["pinned"].as_bool().unwrap_or(false),
            });
        }
        Board { cells }
    }

    /// How many fixtures stand on the grid.
    pub(crate) fn fixtures(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }
}

/// The one letter a chip shows for a fixture word. Not translated: a chip is a
/// glyph, its word is in the tooltip.
pub(crate) fn letter(kind: &str) -> &'static str {
    match kind {
        "wash" => "W",
        "spot" => "S",
        "moving" => "M",
        "backlight" => "B",
        "panel" => "P",
        "continuous" => "C",
        "led_bar" => "L",
        "led_strip" => "T",
        "matrix" => "X",
        _ => "?",
    }
}

/// A fixture word, in the shell's language.
pub(crate) fn kind_name(kind: &str) -> &'static str {
    match kind {
        "wash" => tr("wash"),
        "spot" => tr("spot"),
        "moving" => tr("moving"),
        "backlight" => tr("backlight"),
        "panel" => tr("panel"),
        "continuous" => tr("continuous"),
        "led_bar" => tr("led bar"),
        "led_strip" => tr("led strip"),
        "matrix" => tr("matrix"),
        _ => tr("fixture"),
    }
}

fn row_name(row: Row) -> &'static str {
    match row {
        Row::Back => tr("back row"),
        Row::Mid => tr("mid row"),
        Row::Pro => tr("pro row"),
        Row::House => tr("house row"),
    }
}

fn col_name(col: Col) -> &'static str {
    match col {
        Col::L => tr("left"),
        Col::C => tr("centre"),
        Col::R => tr("right"),
    }
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Floor => tr("floor level"),
        Level::Rig => tr("rig level"),
        Level::Zenith => tr("zenith level"),
    }
}

/// The point a drop at `pos` lands on: the cell that holds it, if any.
pub(crate) fn pin_target(cells: &[(Point, Rect)], pos: Pos2) -> Option<Point> {
    cells.iter().find(|(_, rect)| rect.contains(pos)).map(|(point, _)| *point)
}

/// The `venue.fixtures` node's `hitbox` folder: the venue this node edits, its
/// stage, painted by [`show`].
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
) {
    let venue = venue_map::venue_for(door, gid, node_id, node, graph);
    show(ui, theme, door, &venue.id);
}

/// The stage of venue `venue_id`: the three levels' grids, with every fixture
/// in its cell. A drop on another cell pins that fixture there.
pub(crate) fn show(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str) {
    let Some(detail) = venue_map::venue_detail(door, venue_id) else {
        note(ui, theme, tr("— could not read the venue —"));
        return;
    };
    let board = Board::from_stage(&detail["stage"]);
    if board.fixtures() == 0 {
        note(ui, theme, tr("no fixtures on the grid yet."));
    } else {
        note(ui, theme, tr("drag a fixture to another point to pin it there."));
    }
    // The last failed pin, kept until the next one lands (`ui` temp memory).
    let failed_id = Id::new(("stage-view-failed", venue_id));
    let failed: Option<String> = ui.data(|d| d.get_temp(failed_id));
    if let Some(error) = failed {
        note(ui, theme, &trf("could not pin the fixture: {error}", &[("error", &error)]));
    }

    if let Some((fixture, point)) = paint_grid(ui, theme, &board) {
        let body = json!({
            "op": "fixture", "venue": venue_id,
            "id": fixture, "stage_point": point.word(),
        });
        match door.json("POST", "/venue", Some(body)) {
            Ok(_) => ui.data_mut(|d| {
                d.remove::<String>(failed_id);
            }),
            Err((_, error)) => ui.data_mut(|d| {
                d.insert_temp(failed_id, error);
            }),
        }
        ui.ctx().request_repaint();
    }
}

/// Paints the levels side by side and returns the pin a finished drag asks
/// for, if the chip was dropped on a different cell of the grid.
fn paint_grid(ui: &mut Ui, theme: &Theme, board: &Board) -> Option<(String, Point)> {
    let mut cells: Vec<(Point, Rect)> = Vec::new();
    // (fixture id, pointer position) of a chip whose drag ended this frame.
    let mut dropped: Option<(String, Pos2)> = None;
    ui.horizontal_top(|ui| {
        for level in Level::ALL {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(level_name(level))
                        .font(theme.font_small.clone())
                        .color(theme.ink_weak),
                );
                for row in ROWS {
                    let points: Vec<Point> = COLS
                        .iter()
                        .map(|&col| Point { level, row, col })
                        .filter(Point::valid)
                        .collect();
                    if points.is_empty() {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        for point in points {
                            let rect = cell(ui, theme, point, board, &mut dropped);
                            cells.push((point, rect));
                        }
                    });
                }
            });
        }
    });

    let (fixture, pos) = dropped?;
    let target = pin_target(&cells, pos)?;
    let from = board
        .cells
        .iter()
        .find(|(_, chips)| chips.iter().any(|c| c.fixture == fixture))
        .map(|(point, _)| *point);
    (from != Some(target)).then_some((fixture, target))
}

/// One grid cell: its count, its chips, the point on hover. A cell lit while
/// a chip is dragged over it.
fn cell(
    ui: &mut Ui,
    theme: &Theme,
    point: Point,
    board: &Board,
    dropped: &mut Option<(String, Pos2)>,
) -> Rect {
    let (rect, response) = ui.allocate_exact_size(theme.stage_cell, Sense::hover());
    // Named by its point word, so an accessibility tree (and a kittest) finds it.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, point.word())
    });
    let hot = ui.ctx().dragged_id().is_some() && ui.rect_contains_pointer(rect);
    let fill = if hot { theme.phosphor_dim } else { theme.inset };
    ui.painter().rect_filled(rect, theme.corner_radius(), fill);
    response.on_hover_text(trf(
        "{point} · {row}, {col}",
        &[
            ("point", &point.word()),
            ("row", &row_name(point.row)),
            ("col", &col_name(point.col)),
        ],
    ));

    let chips: &[Chip] = board.cells.get(&point).map(Vec::as_slice).unwrap_or_default();
    let mut inner = ui.new_child(UiBuilder::new().max_rect(rect.shrink(theme.item_gap.x)));
    inner.set_clip_rect(rect);
    inner.horizontal_wrapped(|ui| {
        if !chips.is_empty() {
            ui.label(
                RichText::new(format!("×{}", chips.len()))
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            );
        }
        for chip in chips {
            chip_button(ui, theme, chip, dropped);
        }
    });
    rect
}

/// One fixture: a letter, `*` when hand-tuned, its id and type on hover. A drag
/// that ends records the fixture and where the pointer let go.
fn chip_button(ui: &mut Ui, theme: &Theme, chip: &Chip, dropped: &mut Option<(String, Pos2)>) {
    let (rect, response) = ui.allocate_exact_size(theme.stage_chip, Sense::drag());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, chip.fixture.clone())
    });
    let fill = if chip.pinned { theme.phosphor } else { theme.panel };
    ui.painter().rect_filled(rect, theme.corner_radius(), fill);
    let mark = if chip.tuned { "*" } else { "" };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{}{mark}", letter(&chip.kind)),
        theme.font_small.clone(),
        theme.ink,
    );

    let mut tip = trf(
        "{id} · {kind}",
        &[("id", &chip.fixture), ("kind", &kind_name(&chip.kind))],
    );
    if chip.tuned {
        tip.push('\n');
        tip.push_str(tr("hand-tuned pose"));
    }
    if chip.pinned {
        tip.push('\n');
        tip.push_str(tr("pinned to this point"));
    }
    let ended = response.drag_stopped();
    response.on_hover_text(tip);
    if ended {
        if let Some(pos) = ui.ctx().input(|i| i.pointer.latest_pos()) {
            *dropped = Some((chip.fixture.clone(), pos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage() -> Value {
        json!({
            "name": "stage",
            "placements": [
                { "fixture": "wash-1", "kind": "wash", "point": "l2.back.l", "yaw": 0.0, "pitch": 0.0, "tuned": true, "pinned": true },
                { "fixture": "wash-2", "kind": "wash", "point": "l2.back.l", "yaw": 0.0, "pitch": 0.0, "tuned": false, "pinned": false },
                { "fixture": "move-1", "kind": "moving", "point": "l3.mid.c", "yaw": 0.0, "pitch": 0.0, "tuned": false, "pinned": false },
                { "fixture": "bad", "kind": "spot", "point": "l1.mid.l", "yaw": 0.0, "pitch": 0.0, "tuned": false, "pinned": false },
            ],
            "cells": [],
        })
    }

    /// Placements land in their cells: count and order follow the venue, a point
    /// that is not on the grid is dropped rather than guessed.
    #[test]
    fn the_board_groups_fixtures_by_their_cell() {
        let board = Board::from_stage(&stage());
        let back = Point::parse("l2.back.l").unwrap();
        assert_eq!(board.cells[&back].len(), 2);
        assert_eq!(board.cells[&back][0].fixture, "wash-1");
        assert!(board.cells[&back][0].tuned && board.cells[&back][0].pinned);
        assert_eq!(board.fixtures(), 3);
        assert!(!board.cells.contains_key(&Point { level: Level::Floor, row: Row::Mid, col: Col::L }));
    }

    /// The grid the view paints is the metal-stage grid: 6 floor and rig points
    /// and 9 zenith points, each with a letter and names.
    #[test]
    fn every_grid_point_has_a_letter_and_names() {
        assert_eq!(Point::all().len(), 21);
        for kind in ["wash", "spot", "moving", "backlight", "panel", "continuous", "led_bar", "led_strip", "matrix"] {
            assert_ne!(letter(kind), "?", "{kind} has a letter");
            assert!(!kind_name(kind).is_empty());
        }
        assert_eq!(letter("nonsense"), "?");
    }

    /// A drop lands on the cell under the pointer, and only there.
    #[test]
    fn a_drop_pins_to_the_cell_under_the_pointer() {
        let a = Point::parse("l2.back.l").unwrap();
        let b = Point::parse("l2.back.r").unwrap();
        let cells = vec![
            (a, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(10.0, 10.0))),
            (b, Rect::from_min_max(Pos2::new(12.0, 0.0), Pos2::new(22.0, 10.0))),
        ];
        assert_eq!(pin_target(&cells, Pos2::new(15.0, 5.0)), Some(b));
        assert_eq!(pin_target(&cells, Pos2::new(11.0, 5.0)), None);
    }

    /// The hitbox drag, through egui's pointer: press a chip, carry it over the
    /// grid, let go on another cell. The fixture is pinned there by one
    /// `POST /venue { op: "fixture", stage_point }` through the door.
    #[test]
    fn dragging_a_fixture_to_another_cell_pins_it_through_the_door() {
        use crate::shell::FakeDoor;
        use egui::Vec2;
        use egui_kittest::{kittest::Queryable, Harness};
        use std::rc::Rc;

        let door = Rc::new(FakeDoor::new());
        door.respond("GET", "/venue", json!({ "venue": {}, "stage": stage() }));
        door.respond("POST", "/venue", json!({ "ok": true }));
        let theme = crate::shell::classes::Theme::phosphor();
        let mut harness = Harness::builder()
            .with_size(Vec2::new(
                crate::shell::DESKTOP_VIEWPORT.0 as f32,
                crate::shell::DESKTOP_VIEWPORT.1 as f32,
            ))
            .build_ui(|ui| show(ui, &theme, &door, "stage"));
        harness.run();

        let from = harness.get_by_label("wash-2").rect().center();
        let to = harness.get_by_label("l3.mid.c").rect().center();
        harness.hover_at(from);
        harness.run();
        harness.drag_at(from);
        harness.run();
        for step in 1..=4 {
            let t = step as f32 / 4.0;
            harness.hover_at(from + (to - from) * t);
            harness.run();
        }
        harness.drop_at(to);
        harness.run();

        let pins: Vec<Value> = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .map(|(_, _, params)| serde_json::from_slice(&params.body).unwrap())
            .collect();
        assert_eq!(pins.len(), 1, "one pin for one drop: {pins:?}");
        assert_eq!(pins[0]["op"], "fixture");
        assert_eq!(pins[0]["id"], "wash-2");
        assert_eq!(pins[0]["stage_point"], "l3.mid.c");
    }
}
