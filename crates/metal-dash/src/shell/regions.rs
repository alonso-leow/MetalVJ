//! The splitter grid (the ceding grid of `S.U8`, `plan-opus.md` § O.12.e,
//! reworked): four regions, laid out
//! the way the old HTML dashboard's desktop grid read (deleted 15-sept-2026,
//! S.U18: top row
//! propiedades · grafo, bottom row biblioteca · lanes — `the design notes`
//! § II.5), built from
//! [`egui::Panel::bottom`]/[`egui::Panel::left`]/[`egui::CentralPanel`] — but
//! the old `focus_share`/`cede_share` ceding math is gone. The three
//! boundaries between the four regions are **draggable splitters**: one
//! horizontal (the bottom row's [`egui::Panel::bottom`] with
//! `.resizable(true)`) and one vertical inside each row (each row's
//! [`egui::Panel::left`] with `.resizable(true)`). Each splitter's position
//! persists in egui's own memory under a stable [`egui::Id`] — the theme's
//! shares (`row_share`, `properties_share`, `library_share`) are only a
//! splitter's *first* size, never re-derived from focus, so clicking a
//! region can no longer collapse any other: there is no ceding left, only a
//! split the operator owns. The `region_min` floor is enforced structurally
//! now — `min_size`/`max_size` bound every resizable panel
//! so the four regions can always be seen complete, and egui clamps the
//! drag itself.
//!
//! Two splits, each with one region fixed to an explicit [`egui::Panel`] and
//! its row-mate left to the row's [`egui::CentralPanel`] (which always takes
//! whatever remains): properties/nodes on top, library/lanes on the bottom.
//! Nodes reaching the right edge — `#panel-preview`'s old span folded in —
//! falls out of this for free: nothing sits to nodes' right any more.
//!
//! `S.U9`/`S.U10`/`S.U11`/`S.U12` filled in all four `CentralPanel`/`Panel`
//! halves for real: `panels::nodes::paint`, `panels::lanes::paint`,
//! `panels::library::paint`, and `panels::properties::paint` — `S.U12`'s
//! own job, closing v0.1.

use std::collections::HashMap;

use egui::{CentralPanel, Frame, Id, Panel, Rect, Ui};

use crate::call::Call;

use super::classes::{Arrangement, Placement, Theme};
use super::components::{side_panel, SidebarSide};
use super::frames::Fetcher;
use super::panels::{lanes, library, nodes, properties};
use super::panels::{Region, Selection};

/// Paint the split layout below the header and report the region clicked
/// this frame (if any) plus every region's painted rect — the latter is what
/// `Shell::region_rects` hands the shell tests (`kittest.rs`).
/// `door`/`nodes_state`/
/// `selection` feed the top row's right half (`S.U9`'s graph canvas,
/// `super::panels::nodes::paint`); `lanes_state` feeds the bottom row's
/// right half (`S.U10`'s timeline/mask panel, `super::panels::lanes::paint`);
/// `library_state` feeds the bottom row's left half (`S.U11`'s metraje/
/// colecciones/canciones panel, `super::panels::library::paint`).
#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
    ui: &mut Ui,
    theme: &Theme,
    focus: Option<Region>,
    arrangement: Arrangement,
    active_tab: Region,
    sidebar_open: bool,
    door: &dyn Call,
    fetcher: Option<&Fetcher>,
    nodes_state: &mut nodes::PanelState,
    lanes_state: &mut lanes::PanelState,
    library_state: &mut library::PanelState,
    properties_state: &mut properties::PanelState,
    ingest_state: &mut crate::shell::lanes::IngestLane,
    selection: &mut Selection,
) -> (Option<Region>, HashMap<Region, Rect>) {
    let mut clicked = None;
    let mut rects = HashMap::new();

    if arrangement.is_full_grid() {
        paint_grid(
            ui,
            theme,
            focus,
            door,
            fetcher,
            nodes_state,
            lanes_state,
            library_state,
            properties_state,
            ingest_state,
            selection,
            &mut clicked,
            &mut rects,
        );
        debug_assert!(
            Region::ALL.iter().all(|r| rects.contains_key(r)),
            "every region of Region::ALL should have painted a rect this frame"
        );
    } else {
        paint_delegated(
            ui,
            theme,
            focus,
            arrangement,
            active_tab,
            sidebar_open,
            door,
            fetcher,
            nodes_state,
            lanes_state,
            library_state,
            properties_state,
            ingest_state,
            selection,
            &mut clicked,
            &mut rects,
        );
    }
    (clicked, rects)
}

/// Paint the split layout: the horizontal splitter between rows is the
/// bottom row's [`Panel::bottom`] with `.resizable(true)`; the vertical
/// splitters are each row's left [`Panel::left`] with `.resizable(true)` —
/// all three positions persisted under stable [`egui::Id`]s by egui's own
/// memory. `focus` now only picks which frame draws the accent stroke;
/// nothing about it moves a boundary.
#[allow(clippy::too_many_arguments)]
fn paint_grid(
    ui: &mut Ui,
    theme: &Theme,
    focus: Option<Region>,
    door: &dyn Call,
    fetcher: Option<&Fetcher>,
    nodes_state: &mut nodes::PanelState,
    lanes_state: &mut lanes::PanelState,
    library_state: &mut library::PanelState,
    properties_state: &mut properties::PanelState,
    ingest_state: &mut crate::shell::lanes::IngestLane,
    selection: &mut Selection,
    clicked: &mut Option<Region>,
    rects: &mut HashMap<Region, Rect>,
) {
    let available_height = ui.available_height();
    // First paint only: the theme's row share sizes the split; after that
    // the stored size (egui memory) wins until the operator drags the
    // splitter. The range keeps both rows' interiors above `region_min.y`
    // no matter where the divider is dragged.
    let bottom_height = Theme::clamp_split(available_height, theme.row_share, theme.region_min.y);

    Panel::bottom(Id::new("shell/regions/bottom-row"))
        .resizable(true)
        .default_size(bottom_height)
        .min_size(theme.region_min.y)
        .max_size(available_height - theme.region_min.y)
        .frame(Frame::NONE)
        .show(ui, |ui| {
            paint_bottom_row(
                ui,
                theme,
                focus,
                door,
                nodes_state,
                lanes_state,
                ingest_state,
                library_state,
                selection,
                theme.library_share,
                clicked,
                rects,
            );
        });

    // The bottom row painted first, so this frame's focused overlay button is
    // already known — the properties panel shows it when lanes has the focus
    // (`P5`).
    let overlay_focus = lanes_state.focused_overlay_name().map(str::to_string);
    CentralPanel::default_margins()
        .frame(Frame::NONE)
        .show(ui, |ui| {
            paint_top_row(
                ui,
                theme,
                focus,
                door,
                fetcher,
                nodes_state,
                properties_state,
                ingest_state,
                selection,
                overlay_focus.as_deref(),
                focus == Some(Region::Lanes),
                theme.properties_share,
                clicked,
                rects,
            );
        });
}

/// The delegated layout `S.U13` adds: biblioteca as a left
/// [`side_panel`] (collapsible via `sidebar_open`, the header's own toggle —
/// `header.rs`), then whichever of `grafo`/`lanes` `active_tab` names as the
/// row's `CentralPanel`, and — if `arrangement.library`/`arrangement.tabs`
/// happened to stay `Grid` while only `properties` delegated (a combination
/// today's [`super::classes::arrangement`] thresholds never actually
/// produce, but this function does not assume they can't) — the ordinary
/// top/bottom split for whichever of the three stayed put. Propiedades
/// itself never paints here: once delegated it is `Shell::ui`'s overlay,
/// painted after this whole region on top of everything.
#[allow(clippy::too_many_arguments)]
fn paint_delegated(
    ui: &mut Ui,
    theme: &Theme,
    focus: Option<Region>,
    arrangement: Arrangement,
    active_tab: Region,
    sidebar_open: bool,
    door: &dyn Call,
    fetcher: Option<&Fetcher>,
    nodes_state: &mut nodes::PanelState,
    lanes_state: &mut lanes::PanelState,
    library_state: &mut library::PanelState,
    properties_state: &mut properties::PanelState,
    ingest_state: &mut crate::shell::lanes::IngestLane,
    selection: &mut Selection,
    clicked: &mut Option<Region>,
    rects: &mut HashMap<Region, Rect>,
) {
    let viewport_width = ui.available_width();
    let available_height = ui.available_height();

    if arrangement.library == Placement::Delegated {
        if sidebar_open {
            let width = theme.sidebar_width(viewport_width);
            let (min, max) = theme.sidebar_bounds(viewport_width);
            let panel = side_panel(
                Id::new("shell/regions/library-sidebar"),
                SidebarSide::Left,
                width,
                min,
                max,
            )
            .show(ui, |ui| {
                let response = library::paint(
                    ui,
                    theme,
                    door,
                    library_state,
                    focus == Some(Region::Library),
                );
                if response.clicked() {
                    *clicked = Some(Region::Library);
                }
            });
            rects.insert(Region::Library, panel.response.rect);
        } else {
            rects.insert(
                Region::Library,
                Rect::from_min_size(ui.min_rect().min, egui::Vec2::ZERO),
            );
        }
    }

    CentralPanel::default_margins()
        .frame(Frame::NONE)
        .show(ui, |ui| {
            if arrangement.library == Placement::Grid || arrangement.tabs == Placement::Grid {
                // Not a combination today's thresholds produce (see this
                // function's own doc comment) — fall back to the ordinary
                // grid split for whichever pair stayed together.
                paint_top_row(
                    ui,
                    theme,
                    focus,
                    door,
                    fetcher,
                    nodes_state,
                    properties_state,
                    ingest_state,
                    selection,
                    lanes_state.focused_overlay_name(),
                    focus == Some(Region::Lanes),
                    theme.properties_share,
                    clicked,
                    rects,
                );
                let bottom_height =
                    Theme::clamp_split(ui.available_height(), theme.row_share, theme.region_min.y);
                Panel::bottom(Id::new("shell/regions/bottom-row"))
                    .resizable(true)
                    .default_size(bottom_height)
                    .min_size(theme.region_min.y)
                    .max_size(available_height - theme.region_min.y)
                    .frame(Frame::NONE)
                    .show(ui, |ui| {
                        paint_bottom_row(
                            ui,
                            theme,
                            focus,
                            door,
                            nodes_state,
                            lanes_state,
                            ingest_state,
                            library_state,
                            selection,
                            theme.library_share,
                            clicked,
                            rects,
                        );
                    });
                return;
            }

            // `tabs` delegated: only `active_tab` paints, taking the whole
            // remaining area — the other one is not painted at all this frame,
            // which is also what makes it disappear from the AccessKit tree
            // (`kittest.rs`'s `only_the_active_tab_is_visible`).
            match active_tab {
                Region::Nodes => {
                    if paint_nodes_canvas(
                        ui,
                        theme,
                        door,
                        fetcher,
                        nodes_state,
                        selection,
                        focus == Some(Region::Nodes),
                    ) {
                        *clicked = Some(Region::Nodes);
                    }
                    rects.insert(Region::Nodes, ui.min_rect());
                    rects.insert(
                        Region::Lanes,
                        Rect::from_min_size(ui.min_rect().min, egui::Vec2::ZERO),
                    );
                }
                _ => {
                    let open_graph = nodes_state.selected_graph().map(str::to_string);
                    let response = lanes::paint(
                        ui,
                        theme,
                        door,
                        lanes_state,
                        ingest_state,
                        selection,
                        open_graph.as_deref(),
                        focus == Some(Region::Lanes),
                    );
                    if response.clicked() {
                        *clicked = Some(Region::Lanes);
                    }
                    rects.insert(Region::Lanes, ui.min_rect());
                    rects.insert(
                        Region::Nodes,
                        Rect::from_min_size(ui.min_rect().min, egui::Vec2::ZERO),
                    );
                }
            }
        });

    // Propiedades does not paint a grid cell here at all when delegated —
    // `Shell::ui` paints it as an overlay, on top of everything, once
    // `properties_overlay_open` says to. A zero rect keeps `region_rects`
    // total, for whatever reads it while the grid is not full (nothing does
    // today; `focus_expands_and_others_cede` only runs at `DESKTOP_VIEWPORT`).
    if arrangement.properties == Placement::Delegated {
        rects.insert(
            Region::Properties,
            Rect::from_min_size(ui.min_rect().min, egui::Vec2::ZERO),
        );
    }
}

/// Paint the `graph` region: the node canvas, which is the only state the
/// region has now that the room is a node's own popup (`venue_popup`) rather
/// than a second canvas swapped in by a floating gear. Returns whether the
/// canvas took a click (the region focus check both callers share).
#[allow(clippy::too_many_arguments)]
fn paint_nodes_canvas(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    fetcher: Option<&Fetcher>,
    nodes_state: &mut nodes::PanelState,
    selection: &mut Selection,
    focused: bool,
) -> bool {
    nodes::paint(ui, theme, door, fetcher, nodes_state, selection, focused).clicked()
}

/// The top row: `propiedades` (`S.U12`'s real panel, `super::panels::properties::paint`)
/// on the left, `grafo` (`S.U9`'s real canvas, `super::panels::nodes::paint`)
/// as the row's [`CentralPanel`] on the right. The vertical splitter between
/// them is the left panel's own `.resizable(true)`: `default_share` is only
/// this first paint's width, the dragged position persists in egui's memory,
/// and `max_size` floors **both** halves at `region_min.x` so the row can
/// always be read complete. Same shape as [`paint_bottom_row`] — duplicated
/// rather than parameterised over "which painter" so that neither function
/// has to learn about the other's panel module.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn paint_top_row(
    ui: &mut Ui,
    theme: &Theme,
    focus: Option<Region>,
    door: &dyn Call,
    fetcher: Option<&Fetcher>,
    nodes_state: &mut nodes::PanelState,
    properties_state: &mut properties::PanelState,
    ingest_state: &mut crate::shell::lanes::IngestLane,
    selection: &mut Selection,
    overlay_focus: Option<&str>,
    lanes_focused: bool,
    default_share: f32,
    clicked: &mut Option<Region>,
    rects: &mut HashMap<Region, Rect>,
) {
    let left = Region::Properties;
    let right = Region::Nodes;
    let row_width = ui.available_width();
    let left_width = Theme::clamp_split(row_width, default_share, theme.region_min.x);

    // Read before `nodes::paint` runs this frame — a plain field read, not
    // a paint call, so the order this happens in relative to the
    // `Panel::left`/`CentralPanel` split below (fixed by egui's own layout
    // rules) does not matter. See `nodes::PanelState::selected_graph`'s own
    // doc comment on the one-frame lag this implies for a graph switch.
    let graph_id = nodes_state.selected_graph().map(str::to_string);

    let left_panel = Panel::left(Id::new(format!("{}-col", left.id())))
        .resizable(true)
        .default_size(left_width)
        .min_size(theme.region_min.x)
        .max_size(row_width - theme.region_min.x)
        .frame(Frame::NONE)
        .show(ui, |ui| {
            let response = properties::paint(
                ui,
                theme,
                door,
                properties_state,
                ingest_state,
                &*selection,
                &nodes_state.picked_nodes(&selection),
                graph_id.as_deref(),
                overlay_focus,
                lanes_focused,
                focus == Some(left),
            );
            if response.clicked() {
                *clicked = Some(left);
            }
        });
    rects.insert(left, left_panel.response.rect);

    let right_panel = CentralPanel::default_margins()
        .frame(Frame::NONE)
        .show(ui, |ui| {
            if paint_nodes_canvas(
                ui,
                theme,
                door,
                fetcher,
                nodes_state,
                selection,
                focus == Some(right),
            ) {
                *clicked = Some(right);
            }
        });
    rects.insert(right, right_panel.response.rect);
}

/// The bottom row: `biblioteca` (`S.U11`'s real panel,
/// `super::panels::library::paint`) on the left, `lanes / overlays`
/// (`S.U10`'s real panel, `super::panels::lanes::paint`) as the row's
/// [`CentralPanel`] on the right. The vertical splitter is the left panel's
/// own `.resizable(true)`, persisted in egui's memory exactly like
/// [`paint_top_row`]'s — duplicated for the same reason that function is:
/// this function's only caller (`paint`) is one line different in what it
/// hands its right half, and threading that difference through one shared
/// function would mean it learning about both panel modules and the door at
/// once.
#[allow(clippy::too_many_arguments)]
fn paint_bottom_row(
    ui: &mut Ui,
    theme: &Theme,
    focus: Option<Region>,
    door: &dyn Call,
    nodes_state: &mut nodes::PanelState,
    lanes_state: &mut lanes::PanelState,
    ingest_state: &mut crate::shell::lanes::IngestLane,
    library_state: &mut library::PanelState,
    selection: &Selection,
    default_share: f32,
    clicked: &mut Option<Region>,
    rects: &mut HashMap<Region, Rect>,
) {
    let left = Region::Library;
    let right = Region::Lanes;
    let row_width = ui.available_width();
    let left_width = Theme::clamp_split(row_width, default_share, theme.region_min.x);

    let left_panel = Panel::left(Id::new(format!("{}-col", left.id())))
        .resizable(true)
        .default_size(left_width)
        .min_size(theme.region_min.x)
        .max_size(row_width - theme.region_min.x)
        .frame(Frame::NONE)
        .show(ui, |ui| {
            let response = library::paint(ui, theme, door, library_state, focus == Some(left));
            if response.clicked() {
                *clicked = Some(left);
            }
        });
    rects.insert(left, left_panel.response.rect);

    let right_panel = CentralPanel::default_margins()
        .frame(Frame::NONE)
        .show(ui, |ui| {
            let open_graph = nodes_state.selected_graph().map(str::to_string);
            let response = lanes::paint(
                ui,
                theme,
                door,
                lanes_state,
                ingest_state,
                selection,
                open_graph.as_deref(),
                focus == Some(right),
            );
            if response.clicked() {
                *clicked = Some(right);
            }
        });
    rects.insert(right, right_panel.response.rect);
}
