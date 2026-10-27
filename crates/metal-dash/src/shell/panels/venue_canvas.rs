//! The venue as a **state of the nodes canvas** (22-oct-2026).
//!
//! The room used to be a popup: the gear opened an overlay with a small 3D
//! preview above the add cards. That left the one thing the operator edits —
//! the room, in three dimensions — the size of a thumbnail, and the canvas
//! showing a graph nobody was looking at while it happened. Now the canvas
//! itself flips: [`super::nodes`] allocates its rect, sees
//! [`super::nodes::VenueScene`] set, and paints the whole venue into it here,
//! with a bar at the top carrying `+` (the gallery:
//! [`super::venue_popup::gallery`]) and "Back to nodes" (back to the graph).
//!
//! **The room is still per node.** The scene reads the room the open graph
//! reaches, exactly as the properties panel does
//! ([`super::venue_map::venue_of_graph`]), and the graph never grows a second
//! room picker: the same one room per graph, seen bigger.
//!
//! **Who enters it.** Two doors, both ending in the same flip: the gear on a
//! `venue.surface` node's focus bar (and the Map card's "paint the room"), and
//! the bar's own labelled "Back to nodes" toggle. **Leaving**: `Esc` closes the
//! gallery first, and with no gallery open `Esc` or "Back to nodes" returns to the
//! graph. The canvas keeps [`super::Region::Nodes`] throughout, so focus,
//! hit-testing rules and the properties panel never learn a new region.
//!
//! **Gestures** are the viewport's own [`OrbitCamera`]: left drag orbits, the
//! wheel dollies, the middle drag pans. Nothing here writes the venue except
//! the starter scene below, which posts the same bodies the gallery and the
//! Settings page post (rule 6).

use egui::{Align2, Id, Order, Rect, RichText, Sense, Ui, Vec2};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{icon_button, panel_frame, Icon};
use crate::shell::i18n::tr;

use super::nodes::{PanelState, VenueScene};
use super::venue_map;
use super::venue_popup;
use super::viewport::{self, OrbitCamera, RoomView};
use metal_perform::venue::Venue;

/// The back wall's `z`: where the gallery's own screen cards stand
/// (`venue_popup::add_screen`), so a starter screen is where the next one goes.
pub(crate) const BACK_WALL_Z: f32 = -4.0;

/// How much the wheel dollies the camera per point of scroll.
const DOLLY_STEP: f32 = 0.0025;

/// What the scene's frame did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SceneFrame {
    /// The bar's "Back to nodes", or `Esc` with the gallery shut: back to the graph.
    pub(crate) leave: bool,
    /// The bar's `+` toggled the gallery (its new open/closed state).
    pub(crate) gallery: Option<bool>,
}

/// Whether the room needs a starter scene: no surfaces, no displays and no
/// fixtures. An empty room is not a reason to open a popup — it is the reason
/// to show a floor and one screen.
pub(crate) fn needs_starter(venue: &Venue) -> bool {
    venue.surfaces.is_empty() && venue.displays.is_empty() && venue.fixtures.is_empty()
}

/// Which second row a starter room gets, read off the graph's wired nodes: a
/// `venue.surface` whose `surface` is still empty means the room is about a
/// screen; a `venue.fixtures` node means it is about the rig.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StarterKind {
    #[default]
    Screen,
    Fixtures,
}

/// [`StarterKind`] for the graph: the screen when a `venue.surface` names no
/// surface of the room yet, else the fixtures.
pub(crate) fn starter_kind(graph: &Value) -> StarterKind {
    let nodes = graph["graph"]["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let empty_surface = nodes.iter().any(|n| {
        n["kind"].as_str() == Some(metal_graph::VENUE_SURFACE_KIND)
            && n["config"]["surface"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
    });
    if empty_surface {
        return StarterKind::Screen;
    }
    if nodes
        .iter()
        .any(|n| n["kind"].as_str() == Some(metal_graph::VENUE_FIXTURES_KIND))
    {
        StarterKind::Fixtures
    } else {
        StarterKind::Screen
    }
}

/// Posts the starter scene for `venue_id` through the same routes the gallery
/// uses: a floor `Surface`, then either one 1920×1080 screen 1.92 m wide on
/// the back wall (so the `venue.surface` the graph reaches has something to
/// warp onto), or one fixture row when the graph is about the rig. Returns
/// the bodies it posted, in order.
pub(crate) fn post_starter(door: &dyn Call, venue_id: &str, kind: StarterKind) -> Vec<Value> {
    let floor = json!({
        "op": "surface", "id": venue_id, "surface": "floor",
        "name": tr("floor"), "material": "",
        "origin": [-4.0, 0.0, -4.0],
        "u": [8.0, 0.0, 0.0],
        "v": [0.0, 0.0, 8.0],
    });
    let mut posted = Vec::new();
    if door.json("POST", "/venue", Some(floor.clone())).is_err() {
        return posted;
    }
    posted.push(floor);
    let second = match kind {
        StarterKind::Fixtures => json!({
            "op": "fixture", "venue": venue_id,
            "kind": "wash", "name": tr("wash 1"), "group": "front-wash",
            "wire": "dmx",
        }),
        StarterKind::Screen => json!({
            "op": "screen", "id": venue_id, "surface": "screen-1",
            "screen": {
                "name": tr("screen 1"),
                "width_m": 1.92,
                "height_m": 1.08,
                "pixels": [1920, 1080],
                "kind": "led_wall",
                "link": "local",
                "aspect_lock": "pixels",
                "at": [0.0, 1.08 * 0.5 + 0.6, BACK_WALL_Z],
                "yaw": 0.0,
            },
        }),
    };
    if door.json("POST", "/venue", Some(second.clone())).is_ok() {
        posted.push(second);
    }
    posted
}

/// Paints the scene into `rect` — the canvas's own, already allocated by
/// [`super::nodes::canvas`] — and runs its gestures, bar and gallery.
pub(crate) fn paint(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    rect: Rect,
) -> SceneFrame {
    let mut frame = SceneFrame::default();
    let Some(mut scene) = state.venue_scene.take() else {
        return frame;
    };
    let gid = state.selected_graph().unwrap_or_default().to_string();
    let graph = state.graph_reply();
    let room = venue_map::venue_of_graph(&gid, &graph);
    scene.room = room;

    // Dark ground exactly the canvas's own: the graph's Wire token, so the
    // flip reads as the same canvas showing something else.
    ui.painter()
        .rect_filled(rect, theme.corner_radius(), theme.node_canvas);

    let detail = venue_map::venue_detail(door, &scene.room);
    let mut venue = detail.as_ref().and_then(venue_popup::parse_venue);
    // An empty room gets its starter scene once per session: a floor and one
    // screen (or a fixture row), posted through the gallery's own routes and
    // never a popup. `posted_rooms` is the panel's guard, so a room whose
    // writes failed is not re-posted every frame.
    if let Some(v) = &venue {
        if needs_starter(v) && !state.posted_rooms.contains(&scene.room) {
            state.posted_rooms.insert(scene.room.clone());
            post_starter(door, &scene.room, starter_kind(&graph));
            venue = venue_map::venue_detail(door, &scene.room)
                .as_ref()
                .and_then(venue_popup::parse_venue);
            scene.note = Some(tr("started the room with a floor and a screen").to_string());
        }
    }

    let Some(venue) = venue else {
        paint_room_absent(
            ui, theme, door, state, &mut scene, &mut frame, rect, &gid, &graph,
        );
        state.venue_scene = if frame.leave { None } else { Some(scene) };
        return frame;
    };

    // The camera's default framing comes off the room's own span the first
    // time it paints, so a scene opens on the whole room.
    if !scene.fitted {
        scene.camera = OrbitCamera::room(span_of(&venue));
        scene.fitted = true;
    }

    let gallery_open = state.gallery.is_some();
    scene_gestures(ui, theme, rect, &mut scene, gallery_open);

    // The room, by the viewport's own painter (the same code the properties
    // 3D view runs).
    let view = RoomView {
        camera: &scene.camera,
        masks: &[],
        mask_surface: None,
    };
    viewport::paint_room(&ui.painter_at(rect), theme, rect, &view, &venue);
    // The bar rides over the painted room, not under it: the room's ground
    // fill and its grid are painted first, then the frame and its buttons.
    paint_bar(ui, theme, state, &mut scene, &mut frame, rect);
    if let Some(note) = scene.note.clone() {
        ui.painter().text(
            rect.left_bottom() + theme.item_gap * 2.0,
            Align2::LEFT_BOTTOM,
            note,
            theme.font_small.clone(),
            theme.ink_weak,
        );
    }

    paint_gallery(ui, theme, door, state, &gid, &graph, &mut frame);
    // `Esc`: with the gallery open it belongs to the gallery; with it shut it
    // leaves the scene, exactly like "Back to nodes".
    if !gallery_open && frame.gallery.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        frame.leave = true;
    }
    state.venue_scene = if frame.leave { None } else { Some(scene) };
    frame
}

/// The scene's gestures, the same for the fitted room and the empty one: a
/// left drag orbits, the wheel dollies, the middle drag pans (the
/// `OrbitCamera` methods the viewport already owns). A press on the bar is the
/// bar's, and an open gallery takes the orbit and the wheel for itself.
fn scene_gestures(ui: &Ui, theme: &Theme, rect: Rect, scene: &mut VenueScene, gallery_open: bool) {
    let response = ui.interact(
        rect,
        Id::new("shell/region/nodes/venue-scene"),
        Sense::drag(),
    );
    // A press that landed on the bar is the bar's: it would otherwise read as
    // a scene press through `contains_pointer` and orbit under the buttons.
    let bar_hit = ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| {
        p.y <= rect.top()
            + theme.icon_button_size.y
            + theme.bar_margin().sum().y as f32
            + theme.item_gap.y * 2.0
    });
    if !gallery_open && !bar_hit && response.dragged_by(egui::PointerButton::Primary) {
        scene.camera.orbit(response.drag_delta());
    }
    if !bar_hit && response.dragged_by(egui::PointerButton::Middle) {
        scene.camera.pan(response.drag_delta(), rect);
    }
    // The wheel, read once per frame: Hover of a non-click sense is enough,
    // and the `+`/back bar is a layer above, so a press on it never counts as
    // a scene gesture.
    if !gallery_open && response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            scene.camera.dolly(1.0 + scroll * DOLLY_STEP);
            ui.ctx().request_repaint();
        }
    }
}

/// The scene with no room to read (none on disk yet, or a reply that does not
/// parse): an **empty scene**, not a line of text. The room painter still
/// draws its ground and grid, so the operator sees the space the room will
/// fill, and the bar stays there to leave or to add through the gallery —
/// never a dead canvas, and never a disclaimer where the room should be.
#[allow(clippy::too_many_arguments)]
fn paint_room_absent(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    scene: &mut VenueScene,
    frame: &mut SceneFrame,
    rect: Rect,
    gid: &str,
    graph: &Value,
) {
    let gallery_open = state.gallery.is_some();
    paint_empty_room(ui, theme, rect, scene);
    scene_gestures(ui, theme, rect, scene, gallery_open);
    paint_bar(ui, theme, state, scene, frame, rect);
    paint_gallery(ui, theme, door, state, gid, graph, frame);
}

/// The empty room: a [`Venue`] with its name and nothing in it. Written here
/// rather than derived, so the default is one visible fact — no surfaces, no
/// objects, no displays — and the scene's ground is all the operator sees.
pub(crate) fn empty_venue() -> Venue {
    Venue {
        venue: metal_perform::venue::Meta {
            id: String::new(),
            name: String::new(),
            setting: metal_perform::venue::Setting::Indoor,
            based_on: Vec::new(),
            notes: String::new(),
        },
        shots: Vec::new(),
        surfaces: Vec::new(),
        objects: Vec::new(),
        projectors: Vec::new(),
        fixtures: Vec::new(),
        masks: Vec::new(),
        displays: Vec::new(),
        compositions: Vec::new(),
        sensors: Vec::new(),
    }
}

/// An empty room, painted as a room: the default [`Venue`] (no surfaces, no
/// objects, no displays) through the same painter the fitted scene uses, so
/// the ground and the grid are the only things in it. The camera takes its
/// own default framing once, so a room that gains a wall later opens on the
/// whole of it.
fn paint_empty_room(ui: &Ui, theme: &Theme, rect: Rect, scene: &mut VenueScene) {
    let empty = empty_venue();
    if !scene.fitted {
        scene.camera = OrbitCamera::room(span_of(&empty));
        scene.fitted = true;
    }
    let view = RoomView {
        camera: &scene.camera,
        masks: &[],
        mask_surface: None,
    };
    viewport::paint_room(&ui.painter_at(rect), theme, rect, &view, &empty);
}

/// The scene's bar: `+` (the gallery) and `Back to nodes`.
fn paint_bar(
    ui: &mut Ui,
    theme: &Theme,
    state: &mut PanelState,
    scene: &mut VenueScene,
    frame: &mut SceneFrame,
    rect: Rect,
) {
    let gallery_open = state.gallery.is_some();
    let response = egui::Area::new(Id::new("shell/region/nodes/venue-bar"))
        .order(Order::Middle)
        .fixed_pos(rect.min + theme.item_gap)
        .constrain_to(rect)
        .show(ui.ctx(), |ui| {
            theme
                .card_frame()
                .inner_margin(theme.bar_margin())
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = theme.item_gap;
                    ui.horizontal(|ui| {
                        let plus =
                            icon_button(ui, theme, Icon::Plus, gallery_open, tr("add to the room"));
                        if plus.clicked() {
                            // One state, applied at once: the scene paints the
                            // gallery in this very frame (below the bar), so
                            // the opening click also opens the gallery's
                            // scratch (`venue_gallery`) for it. The frame it
                            // opens on is the guard the click-outside rule
                            // reads, so the `+` press itself never closes it.
                            if gallery_open {
                                state.venue_gallery.close();
                                state.gallery = None;
                            } else {
                                let gid = state.selected_graph().unwrap_or_default().to_string();
                                let node = gallery_node(&state.graph_reply());
                                state.venue_gallery.open(
                                    &gid,
                                    &node,
                                    ui.ctx().cumulative_pass_nr(),
                                );
                                state.gallery = Some(node);
                            }
                            frame.gallery = Some(!gallery_open);
                        }
                        if ui
                            .button(RichText::new(tr("Back to nodes")).font(theme.font_small.clone()))
                            .on_hover_text(tr("back to the node graph"))
                            .clicked()
                        {
                            frame.leave = true;
                        }
                    });
                });
        })
        .response;
    // The bar is named like the gallery's other bars (`new node`), so a
    // kittest can find it by label instead of by glyph codepoint.
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, tr("the room's bar"))
    });
}

/// Paints the gallery over the scene while `state.gallery` is set, and applies
/// what it asked for: Esc, its close button or a click outside close it.
fn paint_gallery(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    state: &mut PanelState,
    gid: &str,
    graph: &Value,
    frame: &mut SceneFrame,
) {
    let Some(node) = state.gallery.clone() else {
        return;
    };
    // The gallery is an overlay on the screen's centre, so its height may
    // exceed the canvas the scene paints into: it is not clamped to `rect`.
    let screen = ui.ctx().content_rect();
    let size = Vec2::new(
        (screen.width() * theme.gallery_share).max(theme.region_min.x),
        (screen.height() * theme.gallery_share).max(theme.region_min.y),
    );
    let Some(value) = graph["graph"]["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|n| n["id"].as_str() == Some(node.as_str()))
        .cloned()
    else {
        state.venue_gallery.close();
        state.gallery = None;
        frame.gallery = Some(false);
        return;
    };
    let room = venue_map::venue_for(door, gid, &node, &value, graph);
    let detail = venue_map::venue_detail(door, &room.id);
    let area = egui::Area::new(Id::new("shell/region/nodes/venue-gallery"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.set_width(size.x);
            panel_frame(theme).show(ui, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                venue_popup::gallery(
                    ui,
                    theme,
                    door,
                    &mut state.venue_gallery,
                    &room.id,
                    &detail,
                    graph,
                );
            });
        });
    let fresh = state.venue_gallery.opened_at() == Some(ui.ctx().cumulative_pass_nr());
    if !state.venue_gallery.is_open()
        || (!fresh && area.response.clicked_elsewhere())
    {
        state.venue_gallery.close();
        state.gallery = None;
        frame.gallery = Some(state.gallery.is_some());
    }
}

/// The node whose room the gallery edits: the graph's own room owner
/// (`venue.fixtures`), else its first `venue.surface` — the same two kinds
/// [`venue_map`] reads a room off, so the gallery's `venue_for` agrees with
/// the scene's `venue_of_graph`.
fn gallery_node(graph: &Value) -> String {
    let nodes = graph["graph"]["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    for kind in [
        metal_graph::VENUE_FIXTURES_KIND,
        metal_graph::VENUE_SURFACE_KIND,
    ] {
        if let Some(id) = nodes
            .iter()
            .find(|n| n["kind"].as_str() == Some(kind))
            .and_then(|n| n["id"].as_str())
        {
            return id.to_string();
        }
    }
    String::new()
}

/// How wide a room is, for the scene camera's default distance — the
/// gallery's own span, so a scene and its preview frame a room the same way.
fn span_of(venue: &Venue) -> f32 {
    venue_popup::span_of(venue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};

    /// One frame of the gestures over `rect`, with `events` as the input.
    fn frame(ctx: &egui::Context, scene: &mut VenueScene, rect: Rect, gallery_open: bool, events: Vec<Event>) {
        let theme = Theme::phosphor();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 900.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            scene_gestures(ui, &theme, rect, scene, gallery_open);
        });
        out.textures_delta.clear();
    }

    /// An empty room (no surfaces, objects or displays) takes the same
    /// gestures as a fitted one: a left drag orbits, a middle drag pans and
    /// the wheel dollies. A gallery that is open takes none of them.
    #[test]
    fn an_empty_room_takes_the_scene_gestures() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(600.0, 400.0));
        let at = Pos2::new(300.0, 300.0);
        let fresh = || VenueScene {
            camera: viewport::OrbitCamera::room(span_of(&empty_venue())),
            room: String::new(),
            selected: None,
            note: None,
            fitted: true,
        };

        let drag = |button: PointerButton, scene: &mut VenueScene, gallery_open: bool| {
            let ctx = egui::Context::default();
            let before = scene.camera.clone();
            frame(&ctx, scene, rect, gallery_open, vec![Event::PointerMoved(at)]);
            frame(
                &ctx,
                scene,
                rect,
                gallery_open,
                vec![Event::PointerButton {
                    pos: at,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            );
            frame(&ctx, scene, rect, gallery_open, vec![Event::PointerMoved(at + Vec2::new(40.0, 0.0))]);
            frame(
                &ctx,
                scene,
                rect,
                gallery_open,
                vec![Event::PointerButton {
                    pos: at + Vec2::new(40.0, 0.0),
                    button,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            (before.yaw, before.target, scene.camera.yaw, scene.camera.target)
        };

        let mut orbit = fresh();
        let (y0, _, y1, _) = drag(PointerButton::Primary, &mut orbit, false);
        assert!(y1 != y0, "a left drag orbits the empty room");

        let mut shut = fresh();
        let (y0, _, y1, _) = drag(PointerButton::Primary, &mut shut, true);
        assert!(y0 == y1, "an open gallery takes the left drag");

        let mut pan = fresh();
        let (_, t0, _, t1) = drag(PointerButton::Middle, &mut pan, false);
        assert!(t0 != t1, "a middle drag pans the empty room");
    }
}
