//! The gallery: one overlay shaped like the settings window and the `+`
//! gallery — the three things a room is made of on the left (**screens**,
//! **projections**, **fixtures**), the choices for the picked one on the
//! right, each a small card.
//!
//! **Where it lives (22-oct-2026).** The room is no longer a popup: the nodes
//! canvas flips into a 3D scene of the whole venue (`super::venue_canvas`),
//! its bar carries a `+` that opens this as a component ([`gallery`]) inside
//! the scene, and the gear on a `venue.surface` (or the Map card) enters that
//! scene. This file only ever *adds* things — a screen, a projection, a
//! fixture — through the one door (`POST /venue`) for the scene's room; the
//! room itself is the scene.
//!
//! **Why cards and not a row of buttons.** The old venue canvas put both
//! gestures behind a text palette and a collapsible form *under* the room, so
//! the one thing the operator came to do (add a screen) scrolled away from the
//! thing they were looking at (the room). This file is the same two-column
//! shape the gallery already taught (`shell::gallery`).
//!
//! **The three lists.**
//!
//! - *Screens* — a resolution paired with a physical size: a preset card is
//!   one `{ pixels, width_m }` pair, and the venue's pixel lock derives the
//!   height. A screen and a projection are the same thing in the venue (a
//!   `Display` on a glass `Surface`); the two lists differ only in whether the
//!   card also stands a lens in front of it.
//! - *Projections* — the same presets plus the projector that throws them: the
//!   card adds the display, then a `Projector` on the room's first object (or
//!   a truss when the room has none) aimed at the new glass. The light source
//!   stays separate from the screen box in the 3D view, which is exactly the
//!   difference the operator asked for.
//! - *Fixtures* — the room's own patch, grouped by universe and read off
//!   `GET /venue`'s `patch` (never recomputed here), then a card that adds one
//!   with the kind, universe and address the operator picked. Nothing invents
//!   a universe: it is the one written into the room.
//!
//! **The gallery, not the room.** The columns *add* things to the room; the
//! room itself is the 3D scene the nodes canvas paints (its bar's room state),
//! and the caller passes that scene the same [`gallery`] this file's
//! [`paint`] wraps. A fixed line of counts (screens, projections, fixtures)
//! sits above the columns so the operator still reads the room's size without
//! a second 3D view fighting the canvas for height.
//!
//! **One fixed size per frame.** [`paint`] computes the popup's outer size
//! once from `ctx.content_rect()` and [`Theme::gallery_share`], hands every
//! inner column and scroll area a *definite* height, and never writes a
//! minimum that depends on `ui.available_height()` of the same frame — the
//! bug that made the popup grow a little every frame a card was hovered or a
//! device selected. [`State::rect`] is the rect the last frame laid out
//! (test-only reader [`State::rect_for_test`]); `the_gallery_keeps_its_height`
//! holds it still across a click.
//!
//! **Reveal, for the scene that owns the popup (S3).** Clicking a
//! `venue.fixture` node beside its row in the fixtures tab is a request to
//! *show that node on the canvas*, never a write: it lands in
//! [`State::reveal`], and the caller drains it once per frame with
//! [`State::take_reveal`] and hands the id to `nodes::PanelState::reveal`
//! (the same reveal the properties panel already uses). The popup never
//! closes on it.
//!
//! **Rule 6.** Every write is `POST /venue` / `POST /graph/node` through
//! [`crate::call::Call`], the same bodies the agent's venue tools post.

use egui::{Align2, Id, Rect, RichText, Sense, Ui, Vec2};
use serde_json::{json, Value};

use glam::Vec3;
use metal_perform::venue::Venue;

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{
    self, icon_button, label_row, nav_list, note, number_row, panel_frame, section_heading, Icon,
};
use crate::shell::i18n::{tr, tr_key, trf};

use super::venue_canvas::BACK_WALL_Z;
use super::viewport;
use super::venue_map;

/// The popup's three lists, in sidebar order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Screens,
    Projections,
    Objects,
    Fixtures,
}

/// The four, in the order the sidebar paints them: what a room is made of —
/// its glass, the light that fills it, the things standing in it, and the rig.
pub const TABS: [Tab; 4] = [Tab::Screens, Tab::Projections, Tab::Objects, Tab::Fixtures];

impl Tab {
    fn icon(self) -> Icon {
        match self {
            Tab::Screens => Icon::Devices,
            Tab::Projections => Icon::Camera,
            Tab::Objects => Icon::Material,
            Tab::Fixtures => Icon::Lamp,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Screens => tr("screens"),
            Tab::Projections => tr("projections"),
            Tab::Objects => tr("room objects"),
            Tab::Fixtures => tr("fixtures"),
        }
    }
}

/// How many cards a row of the popup's grid fits: two, like the gallery's
/// floor, so a card's picture stays wide enough to read its own aspect.
pub const COLUMNS: usize = 2;

/// One resolution/size pair a screen card offers.
#[derive(Clone, Debug)]
pub struct ScreenPreset {
    /// Stable id, never painted: the card's widget salt.
    pub id: &'static str,
    /// English source text, painted through `tr`.
    pub label: &'static str,
    pub width_px: u32,
    pub height_px: u32,
    pub width_m: f32,
    /// The venue's `DisplayKind` word: `led_wall`, `monitor`, `tv`, …
    pub kind: &'static str,
    /// One box, or a structure of panels.
    pub shape: Shape,
}

/// What a card lays in the room: one box, or a structure of panels whose
/// gaps are the way in to the stage. A slice, not a `Vec`, so every preset
/// stays a `const` — the panels are written where the card is declared.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Box,
    Panels(&'static [Panel]),
}

/// One panel of a structure: `pixels` its own grid, `size_m` its own glass,
/// `offset_m` its centre from the structure's centre on the glass plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Panel {
    pub pixels: [u32; 2],
    pub size_m: [f32; 2],
    pub offset_m: [f32; 2],
}

/// The presets: the operator's combos — the two TVs, the three LED widths,
/// 16:10, 32:9, a tall lobby wall and 4:3. Physical sizes are the lit area in
/// metres; the height always comes from the pixel aspect, so a card's own
/// number never disagrees with the venue's lock.
pub const PRESETS: &[ScreenPreset] = &[
    // The televisions, by their own size.
    ScreenPreset {
        id: "tv-32",
        label: tr_key("TV 32\""),
        width_px: 1920,
        height_px: 1080,
        width_m: 0.70,
        kind: "tv",
        shape: Shape::Box,
    },
    ScreenPreset {
        id: "tv-50",
        label: tr_key("TV 50\""),
        width_px: 3840,
        height_px: 2160,
        width_m: 1.10,
        kind: "tv",
        shape: Shape::Box,
    },
    ScreenPreset {
        id: "tv-65",
        label: tr_key("TV 65\""),
        width_px: 3840,
        height_px: 2160,
        width_m: 1.44,
        kind: "tv",
        shape: Shape::Box,
    },
    ScreenPreset {
        id: "tv-75",
        label: tr_key("TV 75\""),
        width_px: 3840,
        height_px: 2160,
        width_m: 1.66,
        kind: "tv",
        shape: Shape::Box,
    },
    ScreenPreset {
        id: "tv-98",
        label: tr_key("TV 98\""),
        width_px: 3840,
        height_px: 2160,
        width_m: 2.17,
        kind: "tv",
        shape: Shape::Box,
    },
    // The structures: how the room's glass is split, and the gaps between
    // the panels are the way in to the stage.
    ScreenPreset {
        id: "strips-3",
        label: tr_key("3 vertical strips"),
        width_px: 1920,
        height_px: 1080,
        width_m: 3.0,
        kind: "led_wall",
        shape: Shape::Panels(&STRIPS_3),
    },
    ScreenPreset {
        id: "strips-5",
        label: tr_key("5 vertical strips"),
        width_px: 1920,
        height_px: 1080,
        width_m: 5.0,
        kind: "led_wall",
        shape: Shape::Panels(&STRIPS_5),
    },
    ScreenPreset {
        id: "floor-square",
        label: tr_key("square floor"),
        width_px: 1024,
        height_px: 1024,
        width_m: 2.0,
        kind: "led_wall",
        shape: Shape::Panels(&FLOOR_SQUARE),
    },
    ScreenPreset {
        id: "wings",
        label: tr_key("winged wall 1024×1536"),
        width_px: 3072,
        height_px: 1536,
        width_m: 3.0,
        kind: "led_wall",
        shape: Shape::Panels(&WINGS),
    },
];

/// The air between two panels of a structure: the way in to the stage.
pub const GAP_M: f32 = 0.25;

/// `n` vertical strips of one 4 m glass, the gap in between each pair.
const fn strips<const N: usize>(n: usize) -> [Panel; N] {
    let glass = 4.0_f32;
    let width = (glass - GAP_M * (n as f32 - 1.0)) / n as f32;
    let mut out = [Panel {
        pixels: [0, 1080],
        size_m: [0.0, 1.08],
        offset_m: [0.0, 0.0],
    }; N];
    let mut i = 0;
    while i < n && i < N {
        out[i] = Panel {
            pixels: [((width / glass) * 1920.0).round() as u32, 1080],
            size_m: [width, 1.08],
            offset_m: [-glass / 2.0 + i as f32 * (width + GAP_M) + width / 2.0, 0.0],
        };
        i += 1;
    }
    out
}

/// The floor, split in four: a square of panels laid flat, a gap on each axis.
const FLOOR_SQUARE: [Panel; 4] = floor_square();

const fn floor_square() -> [Panel; 4] {
    let side = 2.0_f32;
    let cell = (side - GAP_M) / 2.0;
    let mut out = [Panel {
        pixels: [512, 512],
        size_m: [cell, cell],
        offset_m: [0.0, 0.0],
    }; 4];
    let mut j = 0;
    while j < 2 {
        let mut i = 0;
        while i < 2 {
            out[j * 2 + i] = Panel {
                pixels: [512, 512],
                size_m: [cell, cell],
                offset_m: [
                    -side / 2.0 + i as f32 * (cell + GAP_M) + cell / 2.0,
                    side / 2.0 - j as f32 * (cell + GAP_M) - cell / 2.0,
                ],
            };
            i += 1;
        }
        j += 1;
    }
    out
}

/// The operator's wall: the 1024×1536 centre between the two 1024×256 wings,
/// each wing 1.5 m in front of it. The gaps on either side of the centre are
/// the way in to the stage.
pub const WINGS: [Panel; 3] = wings();

const fn wings() -> [Panel; 3] {
    let centre = [1024_u32, 1536];
    let centre_w = 1.0_f32;
    let centre_h = centre_w * centre[1] as f32 / centre[0] as f32;
    let wing = [1024_u32, 256];
    let wing_w = 1.0_f32;
    let wing_h = wing_w * wing[1] as f32 / wing[0] as f32;
    [
        Panel {
            pixels: centre,
            size_m: [centre_w, centre_h],
            offset_m: [0.0, 0.0],
        },
        Panel {
            pixels: wing,
            size_m: [wing_w, wing_h],
            offset_m: [
                -(centre_w + GAP_M + wing_w / 2.0),
                centre_h / 2.0 - wing_h / 2.0,
            ],
        },
        Panel {
            pixels: wing,
            size_m: [wing_w, wing_h],
            offset_m: [
                centre_w + GAP_M + wing_w / 2.0,
                centre_h / 2.0 - wing_h / 2.0,
            ],
        },
    ]
}

const STRIPS_3: [Panel; 3] = strips(3);
const STRIPS_5: [Panel; 5] = strips(5);

/// How near a wing stands to the audience, in metres, when its panel asks to
/// be in front of the wall it belongs to.
pub const WING_FRONT_M: f32 = 1.5;

/// The glass width a section starts at, before the operator types another.
pub const DEFAULT_WIDTH_M: f32 = 2.0;

/// Which node's room the popup edits, and what it remembers between frames.
/// The shell owns one; opening it for another node resets the per-node scratch
/// (the draft name, the throw) but keeps the tab.
#[derive(Default)]
pub struct State {
    open: bool,
    /// The `venue.fixtures` node the room belongs to.
    node: String,
    /// The graph it lives in, read at open so a graph switch closes the popup.
    graph: String,
    tab: Tab,
    /// The name the next added screen gets; empty means "the preset's own".
    name: String,
    /// The glass width the next screen gets, in metres. The card draws this
    /// width, so its second line and the room it writes always agree.
    width_m: f32,
    /// The projection card's throw ratio.
    throw: f32,
    /// The frame the popup opened on, so its own click cannot close it.
    opened_at: Option<u64>,
    note: Option<String>,
    /// The node the operator asked to see on the canvas this frame (a
    /// `venue.fixture` line in the fixtures tab). The caller — the canvas
    /// scene that owns the gallery — drains it with [`State::take_reveal`]
    /// and reveals it. Never a write, never a close.
    reveal: Option<String>,
    /// The popup's outer rect on the last frame it painted: the size
    /// [`paint`] computes once, never grown by the content it lays out.
    rect: Option<Rect>,
}

impl State {
    pub fn open(&mut self, graph: &str, node: &str, frame: u64) {
        if self.node != node || self.graph != graph {
            self.tab = Tab::default();
            self.name.clear();
            self.note = None;
            self.throw = 0.0;
            self.width_m = DEFAULT_WIDTH_M;
        }
        self.graph = graph.to_string();
        self.node = node.to_string();
        self.open = true;
        self.opened_at = Some(frame);
        if self.throw <= 0.0 {
            self.throw = 1.2;
        }
        if self.width_m <= 0.0 {
            self.width_m = DEFAULT_WIDTH_M;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The frame it opened on, so the scene can let its opening click through
    /// without closing it (`gallery`'s click-outside rule).
    pub(crate) fn opened_at(&self) -> Option<u64> {
        self.opened_at
    }

    /// The node whose room the popup edits (empty while closed).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.note = Some(text.into());
    }

    /// The node the operator asked to see on the canvas, once — the caller's
    /// drain (a reveal is a hole to a node, not a write).
    pub fn take_reveal(&mut self) -> Option<String> {
        self.reveal.take()
    }

    /// The tab the columns are on (the canvas scene's narrow slot starts on
    /// screens; a test picks another one through here).
    pub fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
    }

    /// The popup's outer rect on the last painted frame, while it is open.
    #[cfg(test)]
    pub fn rect_for_test(&self) -> Option<Rect> {
        self.rect
    }
}

/// What the popup did this frame.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    None,
    Close,
}

/// Paints the gallery for the room of `state.node` in `gid`, and posts
/// whatever card was clicked. Returns [`Outcome::Close`] on Esc, the close
/// button, a click outside, or when the node it edits is gone.
///
/// This is the standalone door: an overlay of its own at the screen's centre,
/// for callers that hold the `ctx` and nothing else. The venue scene paints
/// [`gallery`] into its own `Ui` instead — the same component, one order down.
pub fn paint(
    ctx: &egui::Context,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    gid: &str,
    graph: &Value,
) -> Outcome {
    if !state.open {
        return Outcome::None;
    }
    let size = popup_size(theme, ctx.content_rect());
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut close = false;
    let area = egui::Area::new(Id::new("shell/venue-popup"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_width(size.x);
            let inner = panel_frame(theme)
                .show(ui, |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    let node = state.node.clone();
                    let node_value = graph["graph"]["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|n| n["id"].as_str() == Some(node.as_str()))
                        .cloned();
                    let Some(value) = node_value else {
                        // The node was removed under the gallery: close
                        // rather than paint a room nobody edits.
                        close = true;
                        return;
                    };
                    let room = venue_map::venue_for(door, gid, &node, &value, graph);
                    let detail = venue_map::venue_detail(door, &room.id);
                    gallery(ui, theme, door, state, &room.id, &detail, graph);
                    if !state.open {
                        close = true;
                    }
                })
                .response;
            state.rect = Some(inner.rect);
        });
    let fresh = state.opened_at == Some(ctx.cumulative_pass_nr());
    if !fresh && (escape || close || area.response.clicked_elsewhere()) {
        state.close();
        return Outcome::Close;
    }
    Outcome::None
}

/// The size the popup takes in a screen `screen`: one read of
/// [`Theme::gallery_share`], floored by the theme's region minimum, shared by
/// the header, the summary line and both columns — never the height the
/// content happens to ask for.
pub fn popup_size(theme: &Theme, screen: Rect) -> Vec2 {
    Vec2::new(
        (screen.width() * theme.gallery_share).max(theme.region_min.x),
        (screen.height() * theme.gallery_share).max(theme.region_min.y),
    )
}

/// The gallery itself, without the popup's frame: the header (room id and the
/// close button), the room's counts, then the tab column and the cards. Both
/// the popup ([`paint`]) and the nodes canvas' venue scene paint this.
///
/// Its height is the size the caller laid out — the popup's [`popup_size`] —
/// read once, never re-read from what the cards laid out: the header, the
/// counts and the columns divide that height between them, and the cards
/// scroll inside their band. The close button writes [`State::close`]; a
/// `venue.fixture` line writes [`State::reveal`] for the caller to drain.
pub fn gallery(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
    graph: &Value,
) {
    let size = Vec2::new(
        ui.available_width(),
        (ui.available_height()).max(theme.region_min.y),
    );
    let laid = ui.max_rect();
    state.rect = Some(laid);
    section_heading(ui, theme, tr("add to the room"));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(venue_id)
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, theme, Icon::Close, false, tr("close")).clicked() {
                state.close();
            }
        });
    });
    ui.add_space(theme.item_gap.y);
    room_summary(ui, theme, detail);
    ui.add_space(theme.item_gap.y);
    let sizes = Vec2::new(
        size.x,
        (size.y - ui.min_rect().height()).max(theme.region_min.y),
    );
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min, sizes)),
        |ui| {
            columns(ui, theme, door, state, venue_id, detail, graph, sizes);
        },
    );
}

/// The two columns inside a box `size` points tall, starting where the
/// cursor is: the tab list, then the cards' band measured once from the box.
/// The band is a scoped child with its own `max_rect`, so the scroll area's
/// viewport is that band and the popup's outer height never follows what the
/// cards ask for.
fn columns(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
    graph: &Value,
    size: Vec2,
) {
    let gap = theme.item_gap.x;
    let side_w = (size.x * 0.3 - gap).max(theme.control_width);
    // The note is a footer band of its own under the columns: its height is
    // reserved here, so the cards' band never runs under it.
    let footer = if state.note.is_some() {
        theme.icon_button_size.y + theme.item_gap.y
    } else {
        0.0
    };
    let rows_h = (size.y - footer).max(theme.region_min.y);
    let origin = ui.cursor().min;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = theme.item_gap;
        ui.allocate_ui_with_layout(
            Vec2::new(side_w, rows_h),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.set_width(side_w);
                ui.set_height(rows_h);
                let rows: Vec<(Icon, &str, Option<String>)> =
                    TABS.iter().map(|t| (t.icon(), t.label(), None)).collect();
                let selected = TABS.iter().position(|t| *t == state.tab);
                if let Some(i) = nav_list(ui, theme, &rows, selected) {
                    state.tab = TABS[i];
                }
            },
        );
        ui.separator();
        let top = ui.cursor().min;
        let cells_w = size.x - (top.x - ui.max_rect().left());
        let rect = Rect::from_min_size(top, Vec2::new(cells_w, rows_h));
        // A top-down column: the scope would otherwise inherit this row's
        // left-to-right layout and lay the cards out sideways, off the popup.
        let column = egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min));
        ui.scope_builder(column, |ui| {
            ui.set_width(cells_w);
            egui::ScrollArea::vertical()
                .id_salt("shell/venue-popup/cards")
                .auto_shrink([false, false])
                .max_height(rows_h)
                .show(ui, |ui| match state.tab {
                    Tab::Screens => screens_tab(ui, theme, door, state, venue_id, detail),
                    Tab::Projections => projections_tab(ui, theme, door, state, venue_id, detail),
                    Tab::Objects => objects_tab(ui, theme, door, state, venue_id, detail),
                    Tab::Fixtures => fixtures_tab(ui, theme, door, state, detail, graph),
                });
        });
    });
    if let Some(note) = state.note.clone() {
        let band = Rect::from_min_size(
            egui::Pos2::new(origin.x, origin.y + rows_h + theme.item_gap.y),
            Vec2::new(size.x, footer - theme.item_gap.y),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(band), |ui| {
            ui.set_clip_rect(band);
            components::note(ui, theme, &note);
        });
    }
}

/// The room in one line: how many screens, projections and fixtures it holds.
/// Read off `GET /venue`'s reply, never recomputed — the 3D room itself is
/// the canvas' scene, so this line is all the popup owes it.
fn room_summary(ui: &mut Ui, theme: &Theme, detail: &Option<Value>) {
    let venue = detail.as_ref().and_then(|d| d.get("venue"));
    let count = |key: &str| {
        venue
            .and_then(|v| v[key].as_array())
            .map(Vec::len)
            .unwrap_or(0)
    };
    let surfaces = count("surface");
    let projections = count("projectors");
    // A projected screen is one `Display` with a lens: count the plain screens
    // as the surfaces no projector output names, so one room never counts the
    // same wall twice.
    let lensed = screens_of_projectors(venue);
    let screens = surfaces.saturating_sub(lensed);
    let text = trf(
        "{screens} screens · {projections} projections · {fixtures} fixtures",
        &[
            ("screens", &screens.to_string()),
            ("projections", &projections.to_string()),
            ("fixtures", &count("fixture").to_string()),
        ],
    );
    ui.label(
        RichText::new(text)
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
}

/// The displays a projector already fills, so a projected screen is never
/// counted as a plain screen: a `Display` whose surface a projector's
/// `output` names.
fn screens_of_projectors(venue: Option<&Value>) -> usize {
    let Some(venue) = venue else {
        return 0;
    };
    let outputs: Vec<&str> = venue["projectors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["output"].as_str())
        .collect();
    venue["displays"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["id"].as_str().is_some_and(|id| outputs.contains(&id)))
        .count()
}

/// The size one card takes in a row `width` points wide, and how tall the row
/// is: the drawing, the two text lines and a margin each way.
fn card_size(ui: &Ui, theme: &Theme, width: f32) -> Vec2 {
    let w = ((width - theme.item_gap.x * (COLUMNS as f32 - 1.0)) / COLUMNS as f32).max(1.0);
    let preview = w * 9.0 / 16.0;
    let lines = ui.text_style_height(&egui::TextStyle::Body)
        + ui.text_style_height(&egui::TextStyle::Small);
    Vec2::new(
        w,
        preview + lines + theme.card_margin().sum().y as f32 + theme.item_gap.y,
    )
}

/// One card: painted into `rect` — the preview drawing, its label and its
/// second line — with the hover accent and the click left to the caller, which
/// owns the rect's response.
fn card(
    ui: &Ui,
    theme: &Theme,
    rect: Rect,
    hovered: bool,
    label: &str,
    sub: &str,
    paint: impl FnOnce(&egui::Painter, Rect),
) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.library_card_corner(), theme.library_card);
    if hovered {
        painter.rect_filled(
            rect,
            theme.library_card_corner(),
            theme.faded(theme.phosphor, theme.hover_alpha),
        );
    }
    let pad = theme.card_margin();
    let inner = Rect::from_min_max(
        rect.min + Vec2::new(f32::from(pad.left), f32::from(pad.top)),
        rect.max - Vec2::new(f32::from(pad.right), f32::from(pad.bottom)),
    );
    let preview = Rect::from_min_size(
        inner.min,
        Vec2::new(inner.width(), inner.width() * 9.0 / 16.0),
    );
    paint(&painter, preview);
    painter.text(
        egui::Pos2::new(inner.left(), preview.bottom() + theme.item_gap.y),
        Align2::LEFT_TOP,
        label,
        theme.font_body.clone(),
        theme.ink,
    );
    painter.text(
        egui::Pos2::new(
            inner.left(),
            preview.bottom() + theme.item_gap.y + ui.text_style_height(&egui::TextStyle::Body),
        ),
        Align2::LEFT_TOP,
        sub,
        theme.font_small.clone(),
        theme.ink_weak,
    );
    let stroke = if hovered {
        theme.accent()
    } else {
        theme.rule_stroke()
    };
    painter.rect_stroke(
        rect,
        theme.library_card_corner(),
        stroke,
        egui::StrokeKind::Inside,
    );
}

/// A row of the preset grid: the cards side by side, and the preset the
/// pointer clicked.
fn preset_row(
    ui: &mut Ui,
    theme: &Theme,
    row: &[&ScreenPreset],
    width_m: f32,
    kind: &str,
) -> Option<ScreenPreset> {
    let width = ui.available_width();
    let size = card_size(ui, theme, width);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, size.y), Sense::hover());
    let mut hit = None;
    for (i, preset) in row.iter().enumerate() {
        let preset = *preset;
        let cell = Rect::from_min_size(
            egui::Pos2::new(
                rect.left() + i as f32 * (size.x + theme.item_gap.x),
                rect.top(),
            ),
            size,
        );
        let response = ui.interact(
            cell,
            Id::new(("shell/venue-popup/card", preset.id, kind)),
            Sense::click(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, preset.label)
        });
        let sub = card_sub(preset, width_m);
        card(
            ui,
            theme,
            cell,
            response.hovered(),
            tr(preset.label),
            &sub,
            |painter, preview| paint_preset(theme, painter, preview, preset, width_m, kind),
        );
        if response.clicked() {
            hit = Some(preset.clone());
        }
    }
    hit
}

/// A card's drawing: the boxes the card will lay in the room, in their own
/// arrangement on the glass plane — one screen, the strips, the square of
/// floor panels or the wall with its two wings. The gaps stay visible: they
/// are the way in to the stage. A projection also draws its lens, apart from
/// the glass.
fn paint_preset(
    theme: &Theme,
    painter: &egui::Painter,
    rect: Rect,
    preset: &ScreenPreset,
    width_m: f32,
    kind: &str,
) {
    painter.rect(
        rect,
        theme.corner_radius(),
        theme.mask_outside,
        theme.rule_stroke(),
        egui::StrokeKind::Inside,
    );
    let boxes = match &preset.shape {
        Shape::Box => vec![([0.0, 0.0], [width_m, height_of(preset, width_m)])],
        Shape::Panels(panels) => panels
            .iter()
            .map(|p| (p.offset_m, [p.size_m[0], p.size_m[1]]))
            .collect(),
    };
    let frame = Rect::from_min_max(
        rect.min,
        egui::Pos2::new(rect.max.x, rect.top() + rect.height() * 0.72),
    );
    let cells = layout_boxes(&boxes, frame);
    let fill = if kind == "projection" {
        theme.faded(theme.phosphor_dim, theme.hover_alpha)
    } else {
        theme.node_header
    };
    for (i, glass) in cells.iter().enumerate() {
        painter.rect_filled(*glass, theme.field_corner(), fill);
        painter.rect_stroke(
            *glass,
            theme.field_corner(),
            theme.accent(),
            egui::StrokeKind::Inside,
        );
        // A wing stands nearer the audience than the wall it belongs to: the
        // card says so with a second edge, the way the 3D view shows depth.
        if preset.id == "wings" && i > 0 {
            let nearer = Rect::from_min_max(
                egui::Pos2::new(glass.left(), glass.top() - theme.hairline),
                glass.max,
            );
            painter.rect_stroke(
                nearer,
                theme.field_corner(),
                theme.line_stroke(theme.phosphor_dim),
                egui::StrokeKind::Outside,
            );
        }
    }
    if kind == "projection" {
        // The lens, apart from the glass: a dot below the screen and the two
        // cone edges — the separation the 3D view keeps between a screen and
        // the light that fills it.
        let lens = egui::Pos2::new(rect.center().x, rect.bottom() - theme.hairline * 4.0);
        painter.circle_filled(lens, theme.hairline * 2.0, theme.phosphor_lit);
        let bounds = cells
            .iter()
            .fold(None, |acc: Option<Rect>, r| {
                Some(acc.map_or(*r, |a| a.union(*r)))
            })
            .unwrap_or(frame);
        for edge in [bounds.left_center(), bounds.right_center()] {
            painter.line_segment([lens, edge], theme.line_stroke(theme.phosphor_dim));
        }
    }
}

/// The boxes of a structure drawn inside `frame`, keeping their arrangement:
/// one scale for both axes so a wall's strips stay strips and the floor's
/// square stays square, centred in the frame.
fn layout_boxes(boxes: &[([f32; 2], [f32; 2])], frame: Rect) -> Vec<Rect> {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for (offset, size) in boxes {
        for axis in 0..2 {
            min[axis] = min[axis].min(offset[axis] - size[axis] / 2.0);
            max[axis] = max[axis].max(offset[axis] + size[axis] / 2.0);
        }
    }
    if !(min[0].is_finite() && max[0] > min[0] && max[1] > min[1]) {
        return Vec::new();
    }
    let span = [max[0] - min[0], max[1] - min[1]];
    let scale = (frame.width() * 0.86 / span[0]).min(frame.height() * 0.86 / span[1]);
    let centre = frame.center();
    boxes
        .iter()
        .map(|(offset, size)| {
            let c = egui::Pos2::new(
                centre.x + (offset[0] - (min[0] + max[0]) / 2.0) * scale,
                centre.y - (offset[1] - (min[1] + max[1]) / 2.0) * scale,
            );
            Rect::from_center_size(c, Vec2::new(size[0] * scale, size[1] * scale))
        })
        .collect()
}

/// A preset's physical aspect: its pixels' width over height.
fn aspect_of(p: &ScreenPreset) -> f32 {
    p.width_px as f32 / p.height_px.max(1) as f32
}

/// The physical height a preset takes at `width_m`, from the pixel aspect — the
/// same lock `POST /venue {op:"screen"}` applies, repeated so the card's second
/// line says the size it will write.
fn height_of(p: &ScreenPreset, width_m: f32) -> f32 {
    width_m * p.height_px as f32 / p.width_px.max(1) as f32
}

/// The room's own document out of `GET /venue`'s reply, with the plural
/// `masks` some replies carry folded onto the singular the model serialises.
pub(crate) fn parse_venue(reply: &Value) -> Option<Venue> {
    let mut value = reply.get("venue")?.clone();
    if value.get("mask").is_none() {
        if let Some(masks) = value.get("masks").cloned() {
            if let Value::Object(map) = &mut value {
                map.insert("mask".to_string(), masks);
            }
        }
    }
    serde_json::from_value(value).ok()
}

/// How wide a room is, for the camera's default distance: the furthest corner
/// of any surface from the origin, with a floor so an empty room still reads.
pub(crate) fn span_of(venue: &Venue) -> f32 {
    let mut span: f32 = 4.0;
    for s in &venue.surfaces {
        for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
            span = span.max(Vec3::from(s.at(u, v)).length());
        }
    }
    for o in &venue.objects {
        span = span.max(Vec3::from(o.at).length());
    }
    span
}

fn screens_tab(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
) {
    ui.label(
        RichText::new(tr("a resolution and the physical size it is built at"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
    if let Some(p) = presets_section(
        ui,
        theme,
        state,
        detail,
        tr("televisions"),
        Kind::Televisions,
    ) {
        add_preset(door, venue_id, &p, state, detail, false);
    }
    ui.add_space(theme.item_gap.y);
    if let Some(p) = presets_section(
        ui,
        theme,
        state,
        detail,
        tr("panel structures"),
        Kind::Structures,
    ) {
        add_preset(door, venue_id, &p, state, detail, false);
    }
}

/// Which half of [`PRESETS`] a section lists.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Televisions,
    Structures,
}

/// One named section: the width the next screen gets, then its cards. The
/// name and the size field are shared, so a section cannot write a screen with
/// a size it did not show.
fn presets_section(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut State,
    detail: &Option<Value>,
    title: &str,
    kind: Kind,
) -> Option<ScreenPreset> {
    section_heading(ui, theme, title);
    let mut width = state.width_m;
    number_row(
        ui,
        theme,
        tr("glass width"),
        &mut width,
        0.3..=12.0,
        "m",
        2,
        true,
    );
    state.width_m = width;
    ui.add_space(theme.item_gap.y);
    let mut added = None;
    let listed: Vec<&ScreenPreset> = PRESETS
        .iter()
        .filter(|p| matches!(p.shape, Shape::Box) == (kind == Kind::Televisions))
        .collect();
    for row in listed.chunks(COLUMNS) {
        if let Some(p) = preset_row(ui, theme, row, state.width_m, "screen") {
            added = Some(p);
        }
    }
    added
}

/// One card's second line: what the card will actually write — the room's own
/// width, or the structure's panel count and its gaps.
fn card_sub(p: &ScreenPreset, width_m: f32) -> String {
    match &p.shape {
        Shape::Box => trf(
            "{w} × {h} m",
            &[
                ("w", &format!("{width_m:.2}")),
                ("h", &format!("{:.2}", height_of(p, width_m))),
            ],
        ),
        Shape::Panels(panels) => trf(
            "{n} panels · {gap} m gaps",
            &[
                ("n", &panels.len().to_string()),
                ("gap", &format!("{GAP_M:.2}")),
            ],
        ),
    }
}

/// Adds one card's screen and, for a structure, one glass per panel. The id
/// reported is the display's; every panel is a surface of its own so the
/// room's grid keeps the air between them.
fn add_preset(
    door: &dyn Call,
    venue_id: &str,
    preset: &ScreenPreset,
    state: &mut State,
    detail: &Option<Value>,
    projector: bool,
) {
    match &preset.shape {
        Shape::Box => match add_screen(
            door,
            venue_id,
            preset,
            &state.name,
            state.width_m,
            detail,
            projector,
            state.throw,
        ) {
            Ok(id) => {
                state.note(trf("added {id}", &[("id", &id)]));
                state.name.clear();
            }
            Err(e) => state.note(e),
        },
        Shape::Panels(panels) => match add_structure(
            door,
            venue_id,
            preset,
            panels,
            &state.name,
            state.width_m,
            detail,
        ) {
            Ok(ids) => {
                let list = ids.join(", ");
                state.note(trf("added {id}", &[("id", &list)]));
                state.name.clear();
            }
            Err(e) => state.note(e),
        },
    }
}

fn projections_tab(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
) {
    ui.label(
        RichText::new(tr(
            "a screen and the lens that throws it, apart from the glass",
        ))
        .font(theme.font_small.clone())
        .color(theme.ink_weak),
    );
    label_row(ui, theme, tr("name"), |ui| {
        ui.text_edit_singleline(&mut state.name)
    });
    let mut throw = state.throw;
    number_row(
        ui,
        theme,
        tr("throw ratio"),
        &mut throw,
        0.5..=4.0,
        ":1",
        2,
        true,
    );
    state.throw = throw;
    let mut width = state.width_m;
    number_row(
        ui,
        theme,
        tr("glass width"),
        &mut width,
        0.3..=12.0,
        "m",
        2,
        true,
    );
    state.width_m = width;
    ui.add_space(theme.item_gap.y);
    let mut added: Option<ScreenPreset> = None;
    let throw_line = trf("throw {r}:1", &[("r", &format!("{:.2}", state.throw))]);
    ui.label(
        RichText::new(&throw_line)
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
    for row in PRESETS.chunks(COLUMNS) {
        let listed: Vec<&ScreenPreset> = row.iter().collect();
        if let Some(p) = preset_row(ui, theme, &listed, state.width_m, "projection") {
            added = Some(p);
        }
    }
    if let Some(p) = added {
        match add_screen(
            door,
            venue_id,
            &p,
            &state.name,
            state.width_m,
            detail,
            true,
            state.throw,
        ) {
            Ok(id) => {
                state.note(trf("projection {id} added", &[("id", &id)]));
                state.name.clear();
            }
            Err(e) => state.note(e),
        }
    }
}

/// One thing an operator puts in a room: the object's kind, the box it
/// occupies and where it stands. A card is this pair — a name and a box — and
/// the room writes it with `POST /venue {op:"object"}` (or `stage`/`truss`/
/// `dummy`, which set the kind and pick a form). A `path` makes the object a
/// swept form along those points (object-local, metres): the truss shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectPreset {
    /// Stable id, never painted: the card's widget salt and the id's stem.
    pub id: &'static str,
    pub label: &'static str,
    /// The venue's `Object.kind` word: `box`, `stage`, `truss`, `dummy`, …
    pub kind: &'static str,
    /// The op that writes it; `object` for a plain box.
    pub op: &'static str,
    pub size_m: [f32; 3],
    /// Where it stands from the room's origin, its centre.
    pub at: [f32; 3],
    /// The swept centre line, object-local; empty for a box.
    pub path: &'static [[f32; 3]],
}

/// The presets: the room's furniture and its rig, in the sizes a venue
/// actually uses. A stage is the platform the audience faces, a bar the counter
/// at the side, a truss hangs the light, a riser lifts a drum kit, a wall hides
/// a wing, a lectern takes the speech. Numbers in metres. For a truss, `size_m`
/// is its extent along the run, then its section (height, width).
pub const OBJECTS: &[ObjectPreset] = &[
    ObjectPreset { id: "stage", label: tr_key("stage platform"), kind: "stage", op: "stage", size_m: [4.0, 0.4, 3.0], at: [0.0, 0.2, -2.0], path: &[] },
    ObjectPreset { id: "riser", label: tr_key("drum riser"), kind: "stage", op: "stage", size_m: [2.0, 0.3, 1.5], at: [0.0, 0.15, -2.8], path: &[] },
    ObjectPreset { id: "steps", label: tr_key("riser steps"), kind: "stage", op: "stage", size_m: [3.0, 0.6, 2.0], at: [0.0, 0.3, -3.0], path: &[] },
    ObjectPreset { id: "thrust", label: tr_key("thrust stage"), kind: "stage", op: "stage", size_m: [5.0, 0.8, 4.0], at: [0.0, 0.4, -1.5], path: &[] },
    ObjectPreset { id: "catwalk", label: tr_key("catwalk"), kind: "stage", op: "stage", size_m: [8.0, 0.1, 0.8], at: [0.0, 2.0, -5.0], path: &[] },
    ObjectPreset { id: "side-fill", label: tr_key("side-fill stage"), kind: "stage", op: "stage", size_m: [2.0, 0.5, 3.0], at: [-4.5, 0.25, -2.0], path: &[] },
    ObjectPreset { id: "bar", label: tr_key("bar counter"), kind: "box", op: "object", size_m: [4.0, 1.1, 0.6], at: [5.0, 0.55, 0.0], path: &[] },
    ObjectPreset { id: "dj", label: tr_key("DJ booth"), kind: "box", op: "object", size_m: [2.4, 1.1, 0.8], at: [3.5, 0.55, -4.0], path: &[] },
    ObjectPreset { id: "backline", label: tr_key("backline"), kind: "box", op: "object", size_m: [5.0, 1.6, 0.6], at: [0.0, 0.8, -5.5], path: &[] },
    ObjectPreset { id: "wall", label: tr_key("side wall"), kind: "box", op: "object", size_m: [0.2, 3.0, 6.0], at: [-4.5, 1.5, -1.0], path: &[] },
    ObjectPreset { id: "lectern", label: tr_key("lectern"), kind: "box", op: "object", size_m: [0.6, 1.1, 0.5], at: [0.0, 0.55, -1.2], path: &[] },
    ObjectPreset { id: "dummy", label: tr_key("figure"), kind: "dummy", op: "dummy", size_m: [0.44, 1.8, 0.44], at: [1.0, 0.9, 0.0], path: &[] },
    ObjectPreset { id: "truss", label: tr_key("lighting truss"), kind: "truss", op: "truss", size_m: [6.0, 0.3, 0.3], at: [0.0, 4.0, -2.0], path: &[] },
    ObjectPreset { id: "truss-3", label: tr_key("straight truss 3 m"), kind: "truss", op: "truss", size_m: [3.0, 0.3, 0.3], at: [0.0, 4.0, -2.0], path: &[] },
    ObjectPreset { id: "truss-12", label: tr_key("straight truss 12 m"), kind: "truss", op: "truss", size_m: [12.0, 0.3, 0.3], at: [0.0, 5.5, -3.0], path: &[] },
    ObjectPreset { id: "truss-corner", label: tr_key("corner truss"), kind: "truss", op: "truss", size_m: [3.0, 0.3, 0.3], at: [0.0, 4.0, -2.0], path: &[[-1.5, 0.0, 0.0], [1.5, 0.0, 0.0], [1.5, 0.0, -1.5]] },
    ObjectPreset { id: "truss-box", label: tr_key("box truss"), kind: "truss", op: "truss", size_m: [4.0, 0.6, 0.6], at: [0.0, 4.5, -2.0], path: &[] },
    ObjectPreset { id: "truss-ladder", label: tr_key("ladder truss"), kind: "truss", op: "truss", size_m: [6.0, 0.2, 0.4], at: [0.0, 3.0, -3.5], path: &[] },
    ObjectPreset { id: "truss-goalpost", label: tr_key("goal-post truss"), kind: "truss", op: "truss", size_m: [4.0, 0.3, 0.3], at: [0.0, 0.0, -3.0], path: &[[-2.0, 0.0, 0.0], [-1.4, 1.4, 0.0], [0.0, 2.0, 0.0], [1.4, 1.4, 0.0], [2.0, 0.0, 0.0]] },
    ObjectPreset { id: "truss-roof", label: tr_key("roof truss"), kind: "truss", op: "truss", size_m: [6.0, 0.3, 0.3], at: [0.0, 3.0, -4.0], path: &[[-3.0, 0.0, 0.0], [0.0, 2.5, 0.0], [3.0, 0.0, 0.0]] },
];

/// The objects tab: the presets, then the room's own objects with a delete.
/// A card adds; the list under it is what the room holds, read off
/// `GET /venue` so it cannot disagree with the scene.
fn objects_tab(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
) {
    ui.label(
        RichText::new(tr("what stands in the room"))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
    label_row(ui, theme, tr("name"), |ui| {
        ui.text_edit_singleline(&mut state.name)
    });
    // The room's own list sits above the cards: the cards fill the scroll
    // band, and a list painted under them is scrolled out of sight.
    objects_list(ui, theme, door, state, venue_id, detail);
    ui.add_space(theme.item_gap.y);
    let mut added: Option<ObjectPreset> = None;
    for row in OBJECTS.chunks(COLUMNS) {
        let width = ui.available_width();
        let size = card_size(ui, theme, width);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, size.y), Sense::hover());
        for (i, preset) in row.iter().enumerate() {
            let cell = Rect::from_min_size(
                egui::Pos2::new(rect.left() + i as f32 * (size.x + theme.item_gap.x), rect.top()),
                size,
            );
            let response = ui.interact(
                cell,
                Id::new(("shell/venue-popup/object", preset.id)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, preset.label)
            });
            let sub = trf(
                "{w} × {h} × {d} m",
                &[
                    ("w", &format!("{:.2}", preset.size_m[0])),
                    ("h", &format!("{:.2}", preset.size_m[1])),
                    ("d", &format!("{:.2}", preset.size_m[2])),
                ],
            );
            card(
                ui,
                theme,
                cell,
                response.hovered(),
                preset.label,
                &sub,
                |painter, preview| paint_object(theme, painter, preview, preset),
            );
            if response.clicked() {
                added = Some(*preset);
            }
        }
    }
    if let Some(preset) = added {
        match add_object(door, venue_id, &preset, &state.name, detail) {
            Ok(id) => {
                state.note(trf("added {id}", &[("id", &id)]));
                state.name.clear();
            }
            Err(e) => state.note(e),
        }
    }
}

/// The room's objects, each with a delete: what the scene shows, in words.
fn objects_list(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    venue_id: &str,
    detail: &Option<Value>,
) {
    let objects = detail
        .as_ref()
        .and_then(|d| d["venue"]["object"].as_array().cloned())
        .unwrap_or_default();
    if objects.is_empty() {
        ui.label(
            RichText::new(tr("— nothing in the room yet —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    }
    section_heading(ui, theme, tr("in the room"));
    for object in &objects {
        let id = object["id"].as_str().unwrap_or("?");
        let kind = object["kind"].as_str().unwrap_or("?");
        // The id the row deletes, named for the pointer and the kittest: the
        // kind is what an operator reads, the id is what the room keys on.
        label_row(ui, theme, id, |ui| {
            ui.label(
                RichText::new(kind)
                    .font(theme.font_small.clone())
                    .color(theme.ink_weak),
            )
        });
        // The delete is its own icon button beside the row, the way the
        // settings lists already carry one. Its label names the object it
        // removes, so a pointer (and a kittest) can tell two rows apart.
        let tip = trf("remove {id} from the room", &[("id", &id)]);
        if icon_button(ui, theme, Icon::Trash, false, &tip).clicked() {
            match delete_object(door, venue_id, id) {
                Ok(()) => state.note(trf("removed {id}", &[("id", &id)])),
                Err(e) => state.note(e),
            }
            return;
        }
    }
}

/// A card's drawing: the object's real shape, projected through the
/// viewport's own camera ([`viewport::OrbitCamera::project`]) so a truss reads
/// as a truss, a riser as steps and a figure as a figure at card size.
/// Boxes draw their visible faces; a truss draws its chords and lattice.
fn paint_object(theme: &Theme, painter: &egui::Painter, rect: Rect, preset: &ObjectPreset) {
    painter.rect(
        rect,
        theme.corner_radius(),
        theme.mask_outside,
        theme.rule_stroke(),
        egui::StrokeKind::Inside,
    );
    let area = rect.shrink(theme.item_gap.x * 2.0);
    let (lo, hi) = extent_of(preset);
    let centre = std::array::from_fn(|a| (lo[a] + hi[a]) * 0.5);
    let span = (0..3).map(|a| hi[a] - lo[a]).fold(0.5_f32, f32::max);
    let mut cam = viewport::OrbitCamera::room(span);
    cam.target = centre;
    cam.distance = span * 2.2;
    let view = Preview {
        painter,
        cam: &cam,
        area,
        eye: cam.eye_point(),
        fill: theme.node_header,
        top: theme.faded(theme.ink, theme.hover_alpha),
        line: theme.rule_stroke(),
    };
    match preset.id {
        "dummy" => view.figure(preset.at, preset.size_m),
        "steps" => {
            // Three treads, each higher than the last towards the back.
            let depth = preset.size_m[2] / 3.0;
            for i in 0..3 {
                let h = preset.size_m[1] * (i + 1) as f32 / 3.0;
                let z = preset.at[2] + preset.size_m[2] * 0.5 - depth * (i as f32 + 0.5);
                view.solid([preset.at[0], h * 0.5, z], [preset.size_m[0], h, depth]);
            }
        }
        "backline" => {
            // Two cabinets side by side: a rig's back line.
            let half = preset.size_m[0] * 0.25;
            for dx in [-half, half] {
                view.solid(
                    [preset.at[0] + dx, preset.at[1], preset.at[2]],
                    [preset.size_m[0] * 0.5, preset.size_m[1], preset.size_m[2]],
                );
            }
        }
        "truss-box" => view.solid(preset.at, preset.size_m),
        _ if preset.op == "truss" => {
            let run: Vec<[f32; 3]> = if preset.path.is_empty() {
                let half = preset.size_m[0] * 0.5;
                vec![
                    [preset.at[0] - half, preset.at[1], preset.at[2]],
                    [preset.at[0] + half, preset.at[1], preset.at[2]],
                ]
            } else {
                preset
                    .path
                    .iter()
                    .map(|p| [p[0] + preset.at[0], p[1] + preset.at[1], p[2] + preset.at[2]])
                    .collect()
            };
            view.lattice(&subdivide(&run, 0.5), preset.size_m[1]);
        }
        _ => view.solid(preset.at, preset.size_m),
    }
}

/// The world box a preset's drawing needs: its own box, grown to take a
/// truss's run.
fn extent_of(preset: &ObjectPreset) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut take = |p: [f32; 3]| {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    };
    let half = std::array::from_fn::<f32, 3, _>(|a| preset.size_m[a] * 0.5);
    for sx in [-1.0_f32, 1.0] {
        for sy in [-1.0_f32, 1.0] {
            for sz in [-1.0_f32, 1.0] {
                take([
                    preset.at[0] + sx * half[0],
                    preset.at[1] + sy * half[1],
                    preset.at[2] + sz * half[2],
                ]);
            }
        }
    }
    for p in preset.path {
        take([p[0] + preset.at[0], p[1] + preset.at[1], p[2] + preset.at[2]]);
    }
    (lo, hi)
}

/// Points along a polyline every `step` metres, so a long run's lattice has
/// a diagonal per bay.
fn subdivide(points: &[[f32; 3]], step: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = (0..3).map(|i| (b[i] - a[i]).powi(2)).sum::<f32>().sqrt();
        let n = (len / step).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f32 / n as f32;
            out.push(std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t));
        }
    }
    if let Some(last) = points.last() {
        out.push(*last);
    }
    out
}

/// One card's preview: the painter, the camera framing it, and the colours
/// its faces and lines take.
struct Preview<'a> {
    painter: &'a egui::Painter,
    cam: &'a viewport::OrbitCamera,
    area: Rect,
    eye: [f32; 3],
    fill: egui::Color32,
    top: egui::Color32,
    line: egui::Stroke,
}

impl Preview<'_> {
    fn project(&self, p: [f32; 3]) -> Option<egui::Pos2> {
        self.cam.project(p, self.area)
    }

    /// A box's faces that the eye can see, the top lit, as filled quads.
    fn solid(&self, centre: [f32; 3], size: [f32; 3]) {
        let h = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
        for axis in 0..3 {
            for sign in [-1.0_f32, 1.0] {
                let plane = centre[axis] + sign * h[axis];
                // A face the eye is behind is never drawn.
                if (self.eye[axis] - plane) * sign <= 0.0 {
                    continue;
                }
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                let mut pts = Vec::with_capacity(4);
                for (su, sv) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    let mut p = [0.0; 3];
                    p[axis] = plane;
                    p[u] = centre[u] + su * h[u];
                    p[v] = centre[v] + sv * h[v];
                    if let Some(q) = self.project(p) {
                        pts.push(q);
                    }
                }
                if pts.len() == 4 {
                    let fill = if axis == 1 && sign > 0.0 { self.top } else { self.fill };
                    self.painter
                        .add(egui::Shape::convex_polygon(pts, fill, self.line));
                }
            }
        }
    }

    /// A truss run: two chords at the section's top and bottom, one diagonal
    /// per bay between them, alternating.
    fn lattice(&self, run: &[[f32; 3]], section_h: f32) {
        let half = section_h * 0.5;
        let up = |p: [f32; 3]| [p[0], p[1] + half, p[2]];
        let down = |p: [f32; 3]| [p[0], p[1] - half, p[2]];
        for (i, pair) in run.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            self.line_between(up(a), up(b));
            self.line_between(down(a), down(b));
            if i % 2 == 0 {
                self.line_between(down(a), up(b));
            } else {
                self.line_between(up(a), down(b));
            }
        }
    }

    fn line_between(&self, a: [f32; 3], b: [f32; 3]) {
        if let (Some(p), Some(q)) = (self.project(a), self.project(b)) {
            self.painter.line_segment([p, q], self.line);
        }
    }

    /// A figure: a body box and a head above it, the head a disc at its
    /// projected radius.
    fn figure(&self, at: [f32; 3], size: [f32; 3]) {
        let body_h = size[1] * 0.7;
        let body_c = [at[0], at[1] - size[1] * 0.5 + body_h * 0.5, at[2]];
        self.solid(body_c, [size[0], body_h, size[2]]);
        let head_r = size[0] * 0.5;
        let head = [at[0], at[1] + size[1] * 0.5 - head_r, at[2]];
        let beside = [head[0] + head_r, head[1], head[2]];
        if let (Some(c), Some(e)) = (self.project(head), self.project(beside)) {
            let r = (e - c).length();
            self.painter.circle_filled(c, r, self.fill);
            self.painter.circle_stroke(c, r, self.line);
        }
    }
}

/// Writes one object preset into the room, through the op it names.
fn add_object(
    door: &dyn Call,
    venue_id: &str,
    preset: &ObjectPreset,
    name: &str,
    detail: &Option<Value>,
) -> Result<String, String> {
    let id = next_object_id(detail, preset);
    let shown = if name.trim().is_empty() {
        preset.label.to_string()
    } else {
        name.trim().to_string()
    };
    let body = match preset.op {
        "stage" | "truss" | "dummy" if !preset.path.is_empty() => json!({
            "op": preset.op, "id": venue_id, "object": id, "name": shown,
            "at": preset.at, "size": preset.size_m, "yaw": 0.0,
            "path": preset.path,
        }),
        "stage" | "truss" | "dummy" => json!({
            "op": preset.op, "id": venue_id, "object": id, "name": shown,
            "at": preset.at, "size": preset.size_m, "yaw": 0.0,
        }),
        _ => json!({
            "op": "object", "id": venue_id, "object": id, "name": shown,
            "kind": preset.kind, "form": "box",
            "at": preset.at, "size": preset.size_m, "yaw": 0.0,
        }),
    };
    door.json("POST", "/venue", Some(body))
        .map_err(|(status, _)| {
            let status = status.to_string();
            trf("venue write failed: {status}", &[("status", &status)])
        })?;
    Ok(id)
}

/// Removes one object from the room.
fn delete_object(door: &dyn Call, venue_id: &str, id: &str) -> Result<(), String> {
    let body = json!({ "op": "object", "id": venue_id, "object": id, "delete": true });
    door.json("POST", "/venue", Some(body))
        .map_err(|(status, _)| {
            let status = status.to_string();
            trf("venue write failed: {status}", &[("status", &status)])
        })?;
    Ok(())
}

/// An object id free in the room: the preset's own stem, numbered.
fn next_object_id(detail: &Option<Value>, preset: &ObjectPreset) -> String {
    let taken: Vec<String> = detail
        .as_ref()
        .and_then(|d| d["venue"]["object"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| o["id"].as_str().map(str::to_string))
        .collect();
    (1..)
        .map(|n| format!("{}-{n}", preset.id))
        .find(|id| !taken.contains(id))
        .unwrap_or_else(|| format!("{}-1", preset.id))
}

/// The fixtures list and the patch card. The list is the room's own patch
/// (`GET /venue`'s `patch`, computed from each profile's channels), grouped by
/// universe, so what the popup shows and what the room holds cannot disagree.
/// The room's **registered** fixtures, by universe: what is already on the
/// bus and wired into the scene. Adding one is a room setting, not a scene
/// edit, so the form lives in Settings; this tab only reads `GET /venue`.
fn fixtures_tab(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut State,
    detail: &Option<Value>,
    graph: &Value,
) {
    let _ = door;
    let patch = detail
        .as_ref()
        .and_then(|d| d["patch"].as_array().cloned())
        .unwrap_or_default();
    if patch.is_empty() {
        ui.label(
            RichText::new(tr("— no fixtures in this room yet —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        note(ui, theme, tr("add fixtures in settings"));
        return;
    }
    let nodes = driving_nodes(graph);
    let mut universes: Vec<u16> = Vec::new();
    for row in &patch {
        let u = row["universe"].as_u64().unwrap_or(0) as u16;
        if !universes.contains(&u) {
            universes.push(u);
        }
    }
    universes.sort_unstable();
    for u in universes {
        section_heading(ui, theme, &trf("universe {n}", &[("n", &u.to_string())]));
        for row in patch
            .iter()
            .filter(|r| r["universe"].as_u64().unwrap_or(0) as u16 == u)
        {
            let id = row["fixture"].as_str().unwrap_or("?");
            let kind = row["kind"].as_str().unwrap_or("?");
            let address = row["address"].as_u64().unwrap_or(0);
            let span = row["span"].as_u64().unwrap_or(0);
            let last = address + span.saturating_sub(1);
            label_row(ui, theme, id, |ui| {
                ui.label(
                    RichText::new(format!("{kind} · {address}–{last}"))
                        .font(theme.font_small.clone())
                        .color(theme.ink),
                )
            });
            // The `venue.fixture` nodes of the open graph that drive this row:
            // a click asks the canvas to show that node (never a write), and
            // the row says when nothing drives it yet.
            let drivers: Vec<&(String, String)> =
                nodes.iter().filter(|(_, row_id)| row_id == id).collect();
            if drivers.is_empty() {
                ui.label(
                    RichText::new(tr("— no node drives it —"))
                        .font(theme.font_small.clone())
                        .color(theme.ink_faint),
                );
            }
            for (node, _) in drivers {
                let label = trf("show node {node}", &[("node", node)]);
                if ui
                    .add(
                        egui::Button::new(RichText::new(&label).font(theme.font_small.clone()))
                            .frame(false),
                    )
                    .on_hover_text(tr("reveal the node on the canvas"))
                    .clicked()
                {
                    state.reveal = Some(node.clone());
                }
            }
        }
    }
}

/// The open graph's `venue.fixture` nodes, `(node id, room row id)`: what
/// drives each registered fixture of the room. Read off `graph["graph"]
/// ["nodes"]`, the same reply the rest of the popup reads.
fn driving_nodes(graph: &Value) -> Vec<(String, String)> {
    graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["kind"].as_str() == Some(metal_graph::VENUE_FIXTURE_KIND))
        .filter_map(|n| {
            let id = n["id"].as_str()?.to_string();
            let row = n["config"]["fixture"].as_str()?.to_string();
            Some((id, row))
        })
        .collect()
}

/// Adds one screen to the room through `POST /venue {op:"screen"}` and, for a
/// projection, the projector that throws it aimed at the new glass. The id it
/// reports is the display's.
fn add_screen(
    door: &dyn Call,
    venue_id: &str,
    preset: &ScreenPreset,
    name: &str,
    width_m: f32,
    detail: &Option<Value>,
    projector: bool,
    throw: f32,
) -> Result<String, String> {
    let height_m = height_of(preset, width_m);
    let id = next_screen_id(detail, preset);
    let shown = if name.trim().is_empty() {
        preset.label.to_string()
    } else {
        name.trim().to_string()
    };
    let count = detail
        .as_ref()
        .and_then(|d| d["venue"]["surface"].as_array().map(Vec::len))
        .unwrap_or(0);
    // Lay the glass on the back wall, stepping right with each one so two
    // screens added in a row never land on top of each other, and down when
    // the wall has run out of room.
    let step = width_m + 0.25;
    let per_row = ((12.0 / step).floor() as usize).max(1);
    let at = [
        (count % per_row) as f32 * step,
        height_m * 0.5 + 0.6 + (count / per_row) as f32 * (height_m + 0.4),
        BACK_WALL_Z,
    ];
    let body = json!({
        "op": "screen", "id": venue_id, "surface": id,
        "screen": {
            "name": shown,
            "width_m": width_m,
            "height_m": height_m,
            "pixels": [preset.width_px, preset.height_px],
            "kind": preset.kind,
            "link": "local",
            "aspect_lock": "pixels",
            "at": at,
            "yaw": 0.0,
        },
    });
    door.json("POST", "/venue", Some(body))
        .map_err(|(status, _)| {
            let status = status.to_string();
            trf("venue write failed: {status}", &[("status", &status)])
        })?;
    if !projector {
        return Ok(id);
    }
    let stand = detail
        .as_ref()
        .and_then(|d| d["venue"]["object"].as_array().and_then(|o| o.first()))
        .and_then(|o| o["id"].as_str())
        .map(str::to_string);
    let stand = match stand {
        Some(id) => id,
        None => {
            // No object to stand on: a truss in the middle of the room, so the
            // lens has a real place in the room rather than a floating pose.
            let body = json!({
                "op": "truss", "id": venue_id, "object": "truss-1", "name": "truss-1",
                "at": [0.0, 3.0, 0.0], "size": [4.0, 0.3, 0.3], "yaw": 0.0,
            });
            let _ = door.json("POST", "/venue", Some(body));
            "truss-1".to_string()
        }
    };
    let body = json!({
        "op": "projector", "id": venue_id, "projector": format!("{id}-lens"),
        "name": format!("{id}-lens"), "output": id, "on": stand,
        "throw_ratio": throw, "aspect": aspect_of(preset),
        "target": at,
    });
    let _ = door.json("POST", "/venue", Some(body));
    Ok(id)
}

/// Adds one structure: every panel its own glass on the back wall, keeping the
/// arrangement the card drew. The air between the panels is written nothing —
/// it stays a gap, which is the way in to the stage. A wing is a stride
/// ([`WING_FRONT_M`]) towards the audience; the rest sit on the wall.
fn add_structure(
    door: &dyn Call,
    venue_id: &str,
    preset: &ScreenPreset,
    panels: &[Panel],
    name: &str,
    width_m: f32,
    detail: &Option<Value>,
) -> Result<Vec<String>, String> {
    let shown = if name.trim().is_empty() {
        tr(preset.label).to_string()
    } else {
        name.trim().to_string()
    };
    let surfaces = detail
        .as_ref()
        .and_then(|d| d["venue"]["surface"].as_array())
        .map(Vec::len)
        .unwrap_or(0);
    // The structure's own scale: the wall it is drawn at, relative to the
    // width the card advertises (a strip wall is 4 m at 3 m typed, so the
    // panels keep their ratio to one another).
    let scale = if preset.width_m > 0.0 {
        width_m / preset.width_m
    } else {
        1.0
    };
    let mut ids = Vec::new();
    for (i, panel) in panels.iter().enumerate() {
        // One structure, one stem: `strips-3-1-2` is the first wall's second
        // strip, so two walls added in a row never share an id.
        let id = format!("{}-{}{}", preset.id, surfaces + 1, panel_suffix(i));
        let height_m = panel.size_m[1] * scale;
        let glass_w = panel.size_m[0] * scale;
        let at = [
            panel.offset_m[0] * scale,
            0.6 + panel.size_m[1] * scale * 0.5 + panel.offset_m[1] * scale,
            BACK_WALL_Z
                - if preset.id == "wings" && i > 0 {
                    WING_FRONT_M
                } else {
                    0.0
                },
        ];
        let body = json!({
            "op": "screen", "id": venue_id, "surface": id,
            "screen": {
                "name": format!("{shown} {}", i + 1),
                "width_m": glass_w,
                "height_m": height_m,
                "pixels": panel.pixels,
                "kind": preset.kind,
                "link": "local",
                "aspect_lock": "pixels",
                "at": at,
                "yaw": 0.0,
            },
        });
        door.json("POST", "/venue", Some(body))
            .map_err(|(status, _)| {
                let status = status.to_string();
                trf("venue write failed: {status}", &[("status", &status)])
            })?;
        ids.push(id);
    }
    Ok(ids)
}

/// A panel's ordinal in its structure, as the id's tail.
fn panel_suffix(i: usize) -> String {
    format!("-{}", i + 1)
}

/// A display id free in the room: the preset's own stem, numbered.
fn next_screen_id(detail: &Option<Value>, preset: &ScreenPreset) -> String {
    let taken: Vec<String> = detail
        .as_ref()
        .and_then(|d| d["venue"]["surface"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["id"].as_str().map(str::to_string))
        .collect();
    (1..)
        .map(|n| format!("{}-{n}", preset.id))
        .find(|id| !taken.contains(id))
        .unwrap_or_else(|| format!("{}-1", preset.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset's physical height follows its pixels — the lock the venue
    /// route applies, so the card cannot promise a size the room refuses.
    #[test]
    fn every_preset_keeps_its_pixel_aspect() {
        for p in PRESETS {
            let h = height_of(p, p.width_m);
            let physical = p.width_m / h;
            assert!(
                (physical - aspect_of(p)).abs() < 1e-4,
                "{}: {}×{} at {} m gives {physical}, want {}",
                p.id,
                p.width_px,
                p.height_px,
                p.width_m,
                aspect_of(p)
            );
        }
    }

    /// The lists are the ones the operator asked for: five televisions, then
    /// the three strips, the square floor and the wall with its two wings.
    #[test]
    fn the_presets_are_the_combinations_the_operator_asked_for() {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        for want in [
            "tv-32", "tv-50", "tv-65", "tv-75", "tv-98", "strips-3", "strips-5", "floor-square", "wings",
        ] {
            assert!(ids.contains(&want), "{want} missing from {ids:?}");
        }
    }

    /// The gallery's two sections are disjoint and cover every card: a
    /// television is one box, a structure is panels.
    #[test]
    fn the_two_sections_split_every_card() {
        let boxes = PRESETS.iter().filter(|p| matches!(p.shape, Shape::Box)).count();
        let panels = PRESETS.len() - boxes;
        assert_eq!(boxes, 5, "five televisions");
        assert_eq!(panels, 4, "four structures");
    }

    /// The operator's wall: a 1024×1536 centre and two 1024×256 wings, the
    /// wings a stride towards the audience, the gaps the way in to the stage.
    #[test]
    fn the_wings_wall_is_the_one_the_operator_described() {
        let preset = PRESETS.iter().find(|p| p.id == "wings").expect("the wall");
        let Shape::Panels(panels) = preset.shape else { panic!("the wall is panels") };
        assert_eq!(panels.len(), 3);
        assert_eq!(panels[0].pixels, [1024, 1536], "the centre is the tall one");
        assert_eq!(panels[1].pixels, [1024, 256], "a wing is the short one");
        assert_eq!(panels[2].pixels, [1024, 256]);
        assert!(panels[1].offset_m[0] < -(panels[0].size_m[0] / 2.0), "the wing clears the centre");
        assert!(panels[2].offset_m[0] > panels[0].size_m[0] / 2.0, "the wing clears the centre");
        assert!(WING_FRONT_M > 1.0, "the wings stand in front of the wall");
    }

    /// The strips tile one glass with a gap between each: what the room holds
    /// is the panels, not one box.
    #[test]
    fn the_strips_leave_their_gaps() {
        let preset = PRESETS.iter().find(|p| p.id == "strips-5").expect("the wall");
        let Shape::Panels(panels) = preset.shape else { panic!("the wall is panels") };
        assert_eq!(panels.len(), 5);
        for pair in panels.windows(2) {
            let right = pair[0].offset_m[0] + pair[0].size_m[0] / 2.0;
            let left = pair[1].offset_m[0] - pair[1].size_m[0] / 2.0;
            assert!(
                (left - right - GAP_M).abs() < 1e-4,
                "a gap of {GAP_M} m, got {}",
                left - right
            );
        }
    }

    /// A new screen id never collides with one in the room.
    #[test]
    fn a_new_screen_id_steps_past_the_rooms_own() {
        let detail = Some(json!({ "venue": { "surface": [{ "id": "tv-50-1" }, { "id": "tv-50-2" }] } }));
        let p = PRESETS.iter().find(|p| p.id == "tv-50").expect("the 50 TV");
        assert_eq!(next_screen_id(&detail, p), "tv-50-3");
        assert_eq!(next_screen_id(&None, p), "tv-50-1");
    }

    /// The tab the popup keeps is reset only when the node changes.
    #[test]
    fn opening_another_node_resets_the_scratch() {
        let mut s = State::default();
        s.open("col", "v-1", 1);
        s.tab = Tab::Fixtures;
        s.name = "back wall".to_string();
        s.open("col", "v-1", 2);
        assert_eq!(s.tab, Tab::Fixtures, "same node keeps the tab");
        assert_eq!(s.name, "back wall");
        s.open("col", "v-2", 3);
        assert_eq!(s.tab, Tab::Screens, "another node starts on screens");
        assert!(s.name.is_empty());
    }

    /// Clicking a gallery card posts the screen(s) to the door.
    #[test]
    fn clicking_a_card_adds_a_screen() {
        use egui_kittest::{kittest::{NodeT, Queryable}, Harness};
        let door = std::rc::Rc::new(crate::shell::FakeDoor::new());
        let theme = Theme::phosphor();
        let mut state = State::default();
        state.open("g", "v-1", 0);
        let d = door.clone();
        let th = theme.clone();
        let mut h = Harness::builder()
            .with_size(Vec2::new(1280.0, 800.0))
            .build_ui_state(
                move |ui, st: &mut State| {
                    gallery(ui, &th, &d, st, "bar", &None, &json!({}));
                },
                state,
            );
        theme.apply(&h.ctx);
        h.ctx.memory_mut(|m| m.options.input_options.max_click_duration = 3600.0);
        h.run_steps(3);
        let posts = |d: &crate::shell::FakeDoor| {
            d.calls
                .borrow()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p == "/venue")
                .count()
        };
        assert_eq!(posts(&door), 0);
        for (label, want) in [(PRESETS[0].label, 1)] {
            let at = h
                .get_all_by_role(egui::accesskit::Role::Button)
                .find(|n| n.accesskit_node().label().is_some_and(|l| l == label))
                .unwrap_or_else(|| panic!("no card {label}"))
                .rect()
                .center();
            h.drag_at(at);
            h.run();
            h.drop_at(at);
            h.run_steps(4);
            assert_eq!(posts(&door), want, "after {label}");
        }
    }
}
