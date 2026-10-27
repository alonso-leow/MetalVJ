//! The venue editor (M4, M5, M6 of the pixel map): the wall-drawing surface
//! of P4.5 grown into the room itself.
//!
//! A top view and a 3D view of the room, handles for yaw/pitch/roll, the shared
//! [`ContourEditor`](super::properties) for surfaces and LED polylines, the
//! per-fixture live state, the patch table and the AI's drafts.
//!
//! The `draw` folder of an `output.device` calls [`draw_section`]: it draws a
//! venue's surfaces on the live device frame or a blank plane with the same
//! [`ContourEditor`] the mask and pin editors use. A closed quad is saved
//! through `POST /venue { op: "surface" }`, so it shows up in every
//! `venue.surface` combo (which reads `GET /venue`).
//!
//! The canvas is the plane the operator draws on: a new surface's four
//! corners map onto the ground plane (`z = 0`), and a picked surface is
//! edited in its own `origin`/`u`/`v` frame so a wall is never flattened.
//! The widget never calls the door; this panel maps its events onto the
//! route, exactly as `mask.rs` and `surface.rs` do.
//!
//! **Rule 6.** Every gesture here posts through [`crate::call::Call`] — the
//! same `POST /venue` ops the `aim_fixture` tool and the routes read. The
//! panel never re-derives a venue: it paints `GET /venue`'s own reply.

use std::collections::HashMap;

use egui::{Id, Rect, RichText, Sense, Ui, Vec2};
use serde_json::{json, Value};

use crate::call::{Call, Params};
use crate::shell::classes::Theme;
use crate::shell::i18n::{tr, trf};

use super::properties::contour::{ContourEditor, ContourEvent, ContourMode};
use super::properties::surface;
use super::properties::PanelState;

/// Grid lines across the blank plane, minor and major alike.
const GRID_DIVISIONS: usize = 20;
/// Grid steps across the canvas's `0..1` space the snap toggle rounds to.
const SNAP_STEPS: f32 = 20.0;
/// Metres the canvas's `0..1` ground plane spans, centred on the origin.
const WORLD_SPAN: f32 = 10.0;

/// One row of the patch table (M4.3): a fixture, the wire it sits on, its
/// universe and address, the slots it spans, and whether those slots overlap
/// another fixture's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PatchRow {
    pub fixture: String,
    pub kind: String,
    pub wire: String,
    pub universe: u16,
    pub address: u16,
    pub span: u16,
    pub conflict: bool,
}

impl PatchRow {
    /// The last slot this fixture writes, 1-based.
    pub fn end(&self) -> u16 {
        self.address.saturating_add(self.span).saturating_sub(1)
    }
}

/// What the panel keeps between frames: the chosen venue and surface, the
/// palette choice, the last clicked fixture, the backdrop and snap toggles,
/// the draw draft, the edited surface's corners and plane basis, the two view
/// rects (for the kittest's click-at-a-point), the LED-polyline draft and the
/// last note.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) venue: Option<String>,
    pub(crate) surface: Option<String>,
    pub(crate) selected: Option<String>,
    /// The fixture whose footprint is highlighted on the preview (M4.2).
    pub(crate) highlight: Option<String>,
    pub(crate) drawing: bool,
    pub(crate) backdrop_camera: bool,
    pub(crate) snap: bool,
    pub(crate) draft: Vec<[f32; 2]>,
    pub(crate) edit_points: Vec<[f32; 2]>,
    pub(crate) edit_basis: Option<([f32; 3], [f32; 3], [f32; 3])>,
    pub(crate) rect: Option<egui::Rect>,
    pub(crate) top: Option<Rect>,
    pub(crate) view: Option<Rect>,
    pub(crate) strip_draft: Vec<[f32; 2]>,
    pub(crate) strip_for: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) frame: crate::shell::frames::Frame,
}

impl State {
    /// The canvas rect as of the last painted frame.
    pub(crate) fn canvas_rect(&self) -> Option<egui::Rect> {
        self.rect
    }
}

/// The panel: load the venue, paint the two views, and post
/// the gesture the operator made.
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    venue_id: &str,
    gid: &str,
    graph: &Value,
    state: &mut State,
) {
    if state.venue.as_deref() != Some(venue_id) {
        state.venue = Some(venue_id.to_string());
        state.selected = None;
        state.strip_for = None;
        state.strip_draft.clear();
    }
    let Some(read) = get_json(door, "/venue", &[("id", venue_id)]) else {
        ui.label(RichText::new(tr("— could not read the venue —")).font(theme.font_small.clone()).color(theme.ink_weak));
        return;
    };
    let venue = read.get("venue").cloned().unwrap_or(Value::Null);
    // F14: each fixture's live swatch, read from the last packet its
    // `venue.fixture` node sent, keyed by fixture id.
    let packets = fixture_packets(door, gid, graph, venue_id);

    // Placing a fixture is the room popup's job now (`venue_popup`): the
    // palette row that lived here was the third place a fixture could be
    // added from, and it never worked from this page.
    top_view(ui, theme, &venue, &packets, state);
    ui.separator();
    view_3d(ui, theme, door, venue_id, &venue, state);
    ui.separator();
    patch_view(ui, theme, &read, state);
    blend_view(ui, theme, door, venue_id, &read, state);
    draft_view(ui, theme, door, venue_id, &read, state);
    if let Some(note) = &state.note {
        ui.label(RichText::new(note.clone()).font(theme.font_small.clone()).color(theme.ink_weak));
    }
}

/// F14: the last packet of every `venue.fixture` node that drives `venue_id`,
/// keyed by fixture id, through the one `GET /venue/fixture-packet` route
/// (rule 6/9). A fixture with no node is absent, so the view keeps its own
/// dot.
fn fixture_packets(
    door: &dyn Call,
    gid: &str,
    graph: &Value,
    venue_id: &str,
) -> std::collections::HashMap<String, Value> {
    let mut out = std::collections::HashMap::new();
    for n in graph["graph"]["nodes"].as_array().into_iter().flatten() {
        if n["kind"].as_str() != Some(metal_graph::VENUE_FIXTURE_KIND) {
            continue;
        }
        if n["config"]["venue"].as_str() != Some(venue_id) {
            continue;
        }
        let (Some(node_id), Some(fixture_id)) = (n["id"].as_str(), n["config"]["fixture"].as_str())
        else {
            continue;
        };
        let mut query = std::collections::HashMap::new();
        query.insert("graph".to_string(), gid.to_string());
        query.insert("node".to_string(), node_id.to_string());
        let params = Params { query, body: Vec::new(), tail: None };
        if let Some(pkt) = door
            .call("GET", "/venue/fixture-packet", params)
            .filter(|r| r.status == 200)
            .and_then(|r| crate::json_memo::parse(&r.body).ok())
        {
            out.insert(fixture_id.to_string(), pkt);
        }
    }
    out
}

/// The `[r, g, b]` a `/venue/fixture-packet` reply carries, if any.
fn packet_rgb(pkt: &Value) -> Option<[f32; 3]> {
    let a = pkt["colour"].as_array()?;
    Some([
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ])
}

/// Paint the `draw` folder of an `output.device`: a backdrop toggle, a
/// surface picker, `+ new surface`, and the shared contour editor over the
/// chosen plane.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    _node: &Value,
    graph: &Value,
    state: &mut PanelState,
) {
    crate::shell::components::section_heading(ui, theme, tr("surface editor"));
    let Some(device) = crate::shell::collection::device_of(gid, graph, Some(node_id)) else {
        ui.label(
            RichText::new(tr("— this node is not on a device graph —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };

    let list = surface::get_venue_list(door);
    let venues: Vec<(String, String)> = list
        .as_ref()
        .and_then(|v| v["venues"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| {
            Some((
                v["id"].as_str()?.to_string(),
                v["name"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();
    let mut venue = state
        .venue_editor
        .venue
        .clone()
        .filter(|v| venues.iter().any(|(id, _)| id == v))
        .or_else(|| list.as_ref().and_then(|v| v["last"].as_str()).map(str::to_string))
        .or_else(|| venues.first().map(|v| v.0.clone()));
    let mut picked_venue = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("venue")).font(theme.font_small.clone()).color(theme.ink_weak));
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/venue-editor-venue"))
            .selected_text(venue.clone().unwrap_or_else(|| "—".into()))
            .show_ui(ui, |ui| {
                for (id, name) in &venues {
                    let label = if name.is_empty() { id.as_str() } else { name.as_str() };
                    if ui.selectable_label(venue.as_deref() == Some(id), label).clicked() && venue.as_deref() != Some(id) {
                        venue = Some(id.clone());
                        picked_venue = true;
                    }
                }
            });
    });
    let Some(venue_id) = venue else {
        ui.label(
            RichText::new(tr("— no venue on disk —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
        return;
    };
    if state.venue_editor.venue.as_deref() != Some(venue_id.as_str()) || picked_venue {
        state.venue_editor.venue = Some(venue_id.clone());
        state.venue_editor.surface = None;
        state.venue_editor.drawing = false;
        state.venue_editor.edit_basis = None;
        state.venue_editor.draft.clear();
        state.venue_editor.note = None;
    }

    let detail = surface::get_venue(door, &venue_id);
    let surface_values: Vec<Value> = detail
        .as_ref()
        .map(surfaces_of)
        .unwrap_or_default();
    let surfaces: Vec<(String, String)> = surface_values
        .iter()
        .filter_map(|s| {
            Some((
                s["id"].as_str()?.to_string(),
                s["name"].as_str().unwrap_or_default().to_string(),
            ))
        })
        .collect();

    let mut picked_surface: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(tr("surface")).font(theme.font_small.clone()).color(theme.ink_weak));
        crate::shell::components::combo_box(theme, Id::new("shell/region/properties/venue-editor-surface"))
            .selected_text(
                state.venue_editor.surface.clone().unwrap_or_else(|| "—".into()),
            )
            .show_ui(ui, |ui| {
                for (id, name) in &surfaces {
                    let label = if name.is_empty() { id.as_str() } else { name.as_str() };
                    if ui
                        .selectable_label(state.venue_editor.surface.as_deref() == Some(id), label)
                        .clicked()
                    {
                        picked_surface = Some(id.clone());
                    }
                }
            });
        if ui.button(RichText::new(tr("+ new surface")).font(theme.font_small.clone())).clicked() {
            state.venue_editor.drawing = true;
            state.venue_editor.surface = None;
            state.venue_editor.edit_basis = None;
            state.venue_editor.draft.clear();
            state.venue_editor.note = None;
        }
    });
    if let Some(id) = picked_surface {
        state.venue_editor.drawing = false;
        state.venue_editor.draft.clear();
        state.venue_editor.surface = Some(id.clone());
        state.venue_editor.edit_points = unit_quad();
        state.venue_editor.edit_basis = surface_values
            .iter()
            .find(|s| s["id"] == json!(id))
            .and_then(basis_of);
        state.venue_editor.note = None;
    }

    ui.horizontal_wrapped(|ui| {
        crate::shell::components::toggle_row(ui, theme, tr("camera frame"), &mut state.venue_editor.backdrop_camera);
        crate::shell::components::toggle_row(ui, theme, tr("snap to grid"), &mut state.venue_editor.snap);
    });

    let outputs = surface::get_outputs(door);
    let (out_w, out_h) = outputs
        .iter()
        .find(|o| o["id"] == json!(device))
        .map(|o| {
            (
                o["width"].as_u64().unwrap_or(1920) as f32,
                o["height"].as_u64().unwrap_or(1080) as f32,
            )
        })
        .unwrap_or((1920.0, 1080.0));
    let aspect = (out_w.max(1.0) / out_h.max(1.0)).clamp(0.2, 5.0);
    let width = theme.mask_canvas_width;
    let size = egui::Vec2::new(width, width / aspect);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    state.venue_editor.rect = Some(rect);

    if state.venue_editor.backdrop_camera {
        let query = vec![
            ("device", device.clone()),
            ("w", (width.round() as u32).to_string()),
        ];
        let pic = state
            .venue_editor
            .frame
            .picture(ui.ctx(), door, "/output.jpg", &query, crate::shell::frames::live_gap());
        match pic {
            Some((id, _)) => {
                let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                ui.painter().image(id, rect, full, theme.image_tint);
            }
            None => {
                ui.painter().rect_filled(rect, theme.corner_radius(), theme.tube);
            }
        }
    } else {
        draw_plane(ui, theme, rect);
    }

    let editing = !state.venue_editor.drawing && state.venue_editor.surface.is_some();
    if state.venue_editor.drawing || editing {
        let mode = if state.venue_editor.drawing { ContourMode::Draw } else { ContourMode::Edit };
        let points_owned: Vec<[f32; 2]> = if editing {
            state.venue_editor.edit_points.clone()
        } else {
            Vec::new()
        };
        let id = ui.id().with("venue-editor-contour");
        let event = ContourEditor {
            points: &points_owned,
            rect,
            mode,
            min_points: 4,
            draft: &mut state.venue_editor.draft,
            lock_count: true,
            id,
        }
        .ui(ui, theme);
        // TODO(P4.4): the plan's "objects" half — `Venue.objects` has no
        // `op: "object"` on `POST /venue` yet, so the same drawing path is
        // not wired to it. Surfaces are the required part.
        handle_event(door, &venue_id, &surface_values, event, state);
    } else {
        ui.label(
            RichText::new(tr("— pick or draw a surface —"))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
        );
    }

    measure(ui, theme, state);
    if let Some(note) = state.venue_editor.note.clone() {
        ui.label(RichText::new(note).font(theme.font_small.clone()).color(theme.ink_weak));
    }
}

/// The AI's draft, as a diff (M5.3): one row per entry the draft adds or
/// removes, with the two buttons that accept it into the venue or throw it
/// away. Nothing here is a second state — the draft lives in
/// `data/venues/<id>.draft.toml` and this paints `GET /venue`'s own `draft`.
fn draft_view(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, read: &Value, state: &mut State) {
    let Some(draft) = read.get("draft").filter(|d| !d.is_null()) else {
        return;
    };
    ui.separator();
    ui.label(RichText::new(tr("draft")).font(theme.font_small.clone()).color(theme.phosphor_lit));
    let rows = draft["diff"].as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        ui.label(RichText::new(tr("no changes")).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    for r in &rows {
        let line = format!(
            "{} {} {}",
            r["change"].as_str().unwrap_or_default(),
            r["kind"].as_str().unwrap_or_default(),
            r["id"].as_str().unwrap_or_default(),
        );
        ui.label(RichText::new(line).font(theme.font_small.clone()).color(theme.ink_weak));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button(RichText::new(tr("accept")).font(theme.font_small.clone())).clicked() {
            if post_draft_op(door, venue_id, "accept").is_some() {
                state.note = Some(tr("draft accepted").to_string());
            }
        }
        if ui.button(RichText::new(tr("reject")).font(theme.font_small.clone())).clicked() {
            if post_draft_op(door, venue_id, "reject").is_some() {
                state.note = Some(tr("draft rejected").to_string());
            }
        }
    });
}

/// The top view: the room's surfaces and objects seen from above, a dot per
/// fixture. A click on a fixture selects it; the 3D view then aims it.
fn top_view(
    ui: &mut Ui,
    theme: &Theme,
    venue: &Value,
    packets: &std::collections::HashMap<String, Value>,
    state: &mut State,
) {
    let rect = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(rect.min, Vec2::new(rect.width().max(64.0), 180.0));
    ui.allocate_rect(rect, Sense::hover());
    state.top = Some(rect);
    let painter = ui.painter();
    painter.rect_filled(rect, theme.corner_radius(), theme.mask_outside);
    let resp = ui.interact(rect, Id::new("venue-editor/top"), Sense::click_and_drag());

    // The fixtures, as dots with their id when selected.
    if let Some(fixtures) = venue["fixture"].as_array() {
        for f in fixtures {
            let at = f["at"].as_array();
            let (x, z) = at
                .and_then(|a| Some((a.first()?.as_f64()? as f32, a.get(2)?.as_f64()? as f32)))
                .unwrap_or((0.0, 0.0));
            let p = rect.min + Vec2::new((x / 8.0) * rect.width(), (-z / 5.0) * rect.height());
            let sel = state.selected.as_deref() == f["id"].as_str();
            // F14: the dot is the fixture's live swatch when a `venue.fixture`
            // node drives it — the colour of its last packet, greyed when the
            // node is bypassed or orphan. With no node it keeps the palette
            // accent, the same dot the editor always drew.
            let pkt = packets.get(f["id"].as_str().unwrap_or_default());
            let muted = matches!(pkt.and_then(|p| p["state"].as_str()), Some("bypassed") | Some("orphan"));
            let colour = pkt.and_then(packet_rgb);
            let (fill, radius) = match (muted, colour, sel) {
                (true, _, _) => (theme.ink_faint, 3.0),
                (false, Some(rgb), true) => (theme.color_of(rgb), 5.0),
                (false, Some(rgb), false) => (theme.color_of(rgb), 3.0),
                (false, None, true) => (theme.phosphor_lit, 5.0),
                (false, None, false) => (theme.accent().color, 3.0),
            };
            painter.circle_filled(p, radius, fill);
        }
    }

    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            // Nearest fixture within a small radius.
            let mut best: Option<(f32, String)> = None;
            if let Some(fixtures) = venue["fixture"].as_array() {
                for f in fixtures {
                    let Some(a) = f["at"].as_array() else { continue };
                    let (x, z) = (a.first().and_then(Value::as_f64).unwrap_or(0.0) as f32, a.get(2).and_then(Value::as_f64).unwrap_or(0.0) as f32);
                    let fp = rect.min + Vec2::new((x / 8.0) * rect.width(), (-z / 5.0) * rect.height());
                    let d = (fp - p).length();
                    if d < 24.0 && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                        best = Some((d, f["id"].as_str().unwrap_or_default().to_string()));
                    }
                }
            }
            state.selected = best.map(|(_, id)| id);
        }
    }
}

/// The 3D view: the back wall as a rectangle; a click with a fixture selected
/// aims it at the point clicked (M4.2), and an LED string in draw mode uses
/// the shared [`ContourEditor`] for its polyline (M4.1).
fn view_3d(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, venue: &Value, state: &mut State) {
    let rect = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(rect.min, Vec2::new(rect.width().max(64.0), 180.0));
    ui.allocate_rect(rect, Sense::hover());
    state.view = Some(rect);
    ui.painter().rect_filled(rect, theme.corner_radius(), theme.mask_outside);

    // The selected fixture's live state, if the route answered one.
    if let Some(id) = state.selected.clone() {
        let state_row = get_json(door, "/venue/state", &[("id", venue_id)])
            .and_then(|s| s["fixtures"].as_array().and_then(|a| a.iter().find(|x| x["fixture"] == json!(id))).cloned());
        if let Some(s) = state_row {
            let c = &s["commanded"];
            let a = &s["actual"];
            ui.label(
                RichText::new(format!(
                    "{id}: pan {:.1}°→{:.1}°  tilt {:.1}°→{:.1}°",
                    c["pan"].as_f64().unwrap_or(0.0),
                    a["pan"].as_f64().unwrap_or(0.0),
                    c["tilt"].as_f64().unwrap_or(0.0),
                    a["tilt"].as_f64().unwrap_or(0.0),
                ))
                .font(theme.font_small.clone())
                .color(theme.ink_weak),
            );
        }
    }

    let drawing = state.strip_for.as_deref() == state.selected.as_deref() && state.selected.is_some();
    if drawing {
        let points = state.strip_draft.clone();
        let event = ContourEditor {
            points: &points,
            rect,
            mode: ContourMode::Draw,
            min_points: 2,
            draft: &mut state.strip_draft,
            lock_count: false,
            id: Id::new("venue-editor/strip"),
        }
        .ui(ui, theme);
        if let ContourEvent::Closed(path) = event {
            if let Some(fixture) = state.selected.clone() {
                if post_path(door, venue_id, &fixture, &path).is_some() {
                    state.note = Some(format!("{} {}", tr("drew path for"), fixture));
                }
            }
            state.strip_for = None;
        }
        return;
    }

    let resp = ui.interact(rect, Id::new("venue-editor/view"), Sense::click_and_drag());
    let target = |p: egui::Pos2| -> [f32; 3] {
        let x = ((p.x - rect.min.x) / rect.width().max(1.0)) * 8.0;
        let y = (1.0 - (p.y - rect.min.y) / rect.height().max(1.0)) * 3.0;
        [x, y, -5.0]
    };
    if let (Some(fixture), true) = (state.selected.clone(), resp.clicked()) {
        if let Some(p) = resp.interact_pointer_pos() {
            if post_aim(door, venue_id, &fixture, target(p)).is_some() {
                state.note = Some(format!("{} {}", tr("aimed"), fixture));
            }
        }
    }
    let _ = venue;
}

/// The patch table (M4.3): every fixture with its wire, universe, address,
/// channel span and any overlap flagged.
fn patch_view(ui: &mut Ui, theme: &Theme, read: &Value, state: &State) {
    ui.label(RichText::new(tr("patch")).font(theme.font_small.clone()).color(theme.ink_weak));
    let rows = patch_rows(read);
    egui::Grid::new("venue-editor/patch").striped(true).show(ui, |ui| {
        for head in ["fixture", "wire", "universe", "address", "span"] {
            ui.label(RichText::new(tr(head)).font(theme.font_small.clone()).color(theme.ink_faint));
        }
        ui.end_row();
        for r in &rows {
            let selected = state.selected.as_deref() == Some(r.fixture.as_str());
            let colour = if r.conflict { theme.phosphor_lit } else if selected { theme.ink } else { theme.ink_weak };
            ui.label(RichText::new(r.fixture.clone()).font(theme.font_small.clone()).color(colour));
            ui.label(RichText::new(r.wire.clone()).font(theme.font_small.clone()).color(colour));
            ui.label(RichText::new(r.universe.to_string()).font(theme.font_small.clone()).color(colour));
            ui.label(RichText::new(r.address.to_string()).font(theme.font_small.clone()).color(colour));
            let span = if r.conflict { format!("{} ", r.span) } else { r.span.to_string() };
            ui.label(RichText::new(span).font(theme.font_small.clone()).color(colour));
            ui.end_row();
        }
    });
}

/// The patch rows of a `GET /venue` reply, with overlaps flagged: two
/// fixtures on one wire and universe whose `[address, address+span-1]`
/// windows touch. Bluetooth fixtures (no slots) are skipped.
pub(crate) fn patch_rows(read: &Value) -> Vec<PatchRow> {
    let mut rows: Vec<PatchRow> = read["patch"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| PatchRow {
                    fixture: r["fixture"].as_str().unwrap_or_default().to_string(),
                    kind: r["kind"].as_str().unwrap_or_default().to_string(),
                    wire: r["wire"].as_str().unwrap_or_default().to_string(),
                    universe: r["universe"].as_u64().unwrap_or(0) as u16,
                    address: r["address"].as_u64().unwrap_or(0) as u16,
                    span: r["span"].as_u64().unwrap_or(0) as u16,
                    conflict: false,
                })
                .collect()
        })
        .unwrap_or_default();
    flag_overlaps(&mut rows);
    rows
}

/// Flags every pair of rows that overlap on the same wire and universe.
pub(crate) fn flag_overlaps(rows: &mut [PatchRow]) {
    for i in 0..rows.len() {
        if rows[i].span == 0 || rows[i].address == 0 {
            continue;
        }
        for j in 0..rows.len() {
            if i == j || rows[j].span == 0 || rows[j].address == 0 {
                continue;
            }
            if rows[i].wire != rows[j].wire || rows[i].universe != rows[j].universe {
                continue;
            }
            let (a0, a1) = (rows[i].address, rows[i].end());
            let (b0, b1) = (rows[j].address, rows[j].end());
            if a0 <= b1 && b0 <= a1 {
                rows[i].conflict = true;
            }
        }
    }
}

/// `POST /venue { op: "aim", id, fixture, target }` — click-to-aim (M4.2).
pub(crate) fn post_aim(door: &dyn Call, venue: &str, fixture: &str, target: [f32; 3]) -> Option<Value> {
    door.json("POST", "/venue", Some(json!({ "op": "aim", "id": venue, "fixture": fixture, "target": target }))).ok()
}

/// `POST /venue { op: "path", id, fixture, path }` — an LED polyline drawn
/// with the shared contour editor (M4.1).
pub(crate) fn post_path(door: &dyn Call, venue: &str, fixture: &str, path: &[[f32; 2]]) -> Option<Value> {
    door.json("POST", "/venue", Some(json!({ "op": "path", "id": venue, "fixture": fixture, "path": path }))).ok()
}

/// `POST /venue { op: "draft" }`'s accept/reject (M5.3): the diff view's two
/// buttons.
pub(crate) fn post_draft_op(door: &dyn Call, venue: &str, op: &str) -> Option<Value> {
    door.json("POST", "/venue", Some(json!({ "op": op, "id": venue }))).ok()
}

/// P-F: edge blending, one row per surface with ≥ 2 pinned projectors
/// (`GET /venue`'s `blend`): the "Fusionar bordes" toggle (`POST /venue
/// {op:"blend", surface, enabled}`), a sketch of the surface with every
/// overlap hatched, and a test pattern button per projector (`POST /overlay
/// {op:"solid", pattern:"grid"}` on its output).
fn blend_view(ui: &mut Ui, theme: &Theme, door: &dyn Call, venue_id: &str, read: &Value, state: &mut State) {
    let Some(surfaces) = read["blend"].as_object().filter(|s| !s.is_empty()) else {
        return;
    };
    ui.separator();
    ui.label(RichText::new(tr("edge blending")).font(theme.font_small.clone()).color(theme.ink_weak));
    for (sid, row) in surfaces {
        let mut on = row["enabled"].as_bool().unwrap_or(false);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(sid.as_str()).font(theme.font_small.clone()));
            if crate::shell::components::toggle_row(ui, theme, tr("blend edges"), &mut on).changed()
                && post_blend(door, venue_id, sid, on).is_some()
            {
                let sid: &dyn std::fmt::Display = sid;
                state.note = Some(if on {
                    trf("edge blending on for {surface}", &[("surface", sid)])
                } else {
                    trf("edge blending off for {surface}", &[("surface", sid)])
                });
            }
        });
        let rect = ui.available_rect_before_wrap();
        let rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), crate::shell::classes::BLEND_CANVAS_HEIGHT));
        ui.allocate_rect(rect, Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, theme.corner_radius(), theme.mask_outside);
        let to_screen = |p: &Value| -> Option<egui::Pos2> {
            let a = p.as_array()?;
            Some(rect.min + Vec2::new(a.first()?.as_f64()? as f32 * rect.width(), a.get(1)?.as_f64()? as f32 * rect.height()))
        };
        for o in row["overlaps"].as_array().into_iter().flatten() {
            let poly: Vec<egui::Pos2> = o["points"].as_array().into_iter().flatten().filter_map(to_screen).collect();
            if poly.len() < 3 {
                continue;
            }
            hatch(&painter, theme, &poly);
            painter.add(egui::Shape::closed_line(poly, theme.accent()));
        }
        painter.rect_stroke(rect, theme.corner_radius(), theme.rule_stroke(), egui::StrokeKind::Inside);
        ui.horizontal_wrapped(|ui| {
            for p in row["projectors"].as_array().into_iter().flatten() {
                let (Some(id), Some(output)) = (p["id"].as_str(), p["output"].as_str()) else { continue };
                let label = trf("test pattern {projector}", &[("projector", &id)]);
                if ui.button(RichText::new(label).font(theme.font_small.clone())).clicked()
                    && post_test_pattern(door, output).is_some()
                {
                    state.note = Some(trf("test pattern on {output}", &[("output", &output)]));
                }
            }
        });
    }
}

/// Diagonal lines across the convex `poly`, `BLEND_HATCH_STEP` apart: each
/// line `x + y = c` clipped to the polygon.
fn hatch(painter: &egui::Painter, theme: &Theme, poly: &[egui::Pos2]) {
    let step = crate::shell::classes::BLEND_HATCH_STEP;
    let (lo, hi) = poly.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.x + p.y), hi.max(p.x + p.y)));
    let stroke = theme.line_stroke(theme.warn);
    let mut c = lo + step;
    while c < hi {
        let mut hits: Vec<egui::Pos2> = Vec::new();
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            let (fa, fb) = (a.x + a.y - c, b.x + b.y - c);
            if (fa <= 0.0) != (fb <= 0.0) {
                let t = fa / (fa - fb);
                hits.push(a + (b - a) * t);
            }
        }
        if hits.len() >= 2 {
            painter.line_segment([hits[0], hits[1]], stroke);
        }
        c += step;
    }
}

/// P-F `POST /venue {op:"blend"}`: every projector pinned to `surface`.
pub(crate) fn post_blend(door: &dyn Call, venue: &str, surface: &str, enabled: bool) -> Option<Value> {
    door.json("POST", "/venue", Some(json!({ "op": "blend", "id": venue, "surface": surface, "enabled": enabled }))).ok()
}

/// P-F: the white + grid test pattern on one projector's output.
pub(crate) fn post_test_pattern(door: &dyn Call, output: &str) -> Option<Value> {
    door.json(
        "POST",
        "/overlay",
        Some(json!({ "op": "solid", "device": output, "rgb": [255, 255, 255], "pattern": "grid",
                     "duration_seconds": crate::shell::classes::TEST_PATTERN_SECONDS })),
    )
    .ok()
}

/// A `GET` with a query string through the same door every panel uses (the
/// trait's `json` carries no query, so this builds [`Params`] directly).
fn get_json(door: &dyn Call, path: &str, query: &[(&str, &str)]) -> Option<Value> {
    let params = Params {
        query: query.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect::<HashMap<_, _>>(),
        body: Vec::new(),
        tail: None,
    };
    match door.call("GET", path, params) {
        Some(reply) if reply.status == 200 => crate::json_memo::parse(&reply.body).ok(),
        _ => None,
    }
}

/// The blank plane: a [`Theme`]-coloured grid and the two origin axes.
fn draw_plane(ui: &mut Ui, theme: &Theme, rect: egui::Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme.corner_radius(), theme.tube);
    for i in 1..GRID_DIVISIONS {
        let t = i as f32 / GRID_DIVISIONS as f32;
        let x = rect.left() + rect.width() * t;
        let y = rect.top() + rect.height() * t;
        painter.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], theme.rule_stroke());
        painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], theme.rule_stroke());
    }
    let centre = rect.center();
    painter.line_segment(
        [egui::pos2(centre.x, rect.top()), egui::pos2(centre.x, rect.bottom())],
        theme.accent(),
    );
    painter.line_segment(
        [egui::pos2(rect.left(), centre.y), egui::pos2(rect.right(), centre.y)],
        theme.accent(),
    );
    painter.rect_stroke(rect, theme.corner_radius(), theme.rule_stroke(), egui::StrokeKind::Inside);
}

/// Maps one editor frame onto the route: a closed quad creates a surface, a
/// changed quad re-points the picked one, and `Esc` cancels the draft.
fn handle_event(door: &dyn Call, venue: &str, surfaces: &[Value], event: ContourEvent, state: &mut PanelState) {
    match event {
        ContourEvent::Closed(pts) => {
            let pts = snap_all(pts, state.venue_editor.snap);
            let id = fresh_surface_id(surfaces);
            let world: Vec<[f32; 3]> = pts.iter().map(|n| plane_point(*n)).collect();
            post_surface(door, venue, &id, Some(&id), &world);
            state.venue_editor.note = Some(trf("surface created: {id}", &[("id", &id)]));
            state.venue_editor.drawing = false;
            state.venue_editor.draft.clear();
            state.venue_editor.surface = None;
        }
        ContourEvent::Changed(pts) => {
            let basis = state.venue_editor.edit_basis;
            let Some(sid) = state.venue_editor.surface.clone() else { return };
            let Some((origin, u, v)) = basis else { return };
            let pts = snap_all(pts, state.venue_editor.snap);
            let world: Vec<[f32; 3]> = pts
                .iter()
                .map(|n| add3(origin, add3(scale3(u, n[0]), scale3(v, n[1]))))
                .collect();
            post_surface(door, venue, &sid, None, &world);
            state.venue_editor.edit_points = pts;
            state.venue_editor.note = Some(tr("surface moved").to_string());
        }
        ContourEvent::Cancelled => {
            state.venue_editor.drawing = false;
            state.venue_editor.draft.clear();
        }
        ContourEvent::None => {}
    }
}

/// The quad's own size in metres, under the editor.
fn measure(ui: &mut Ui, theme: &Theme, state: &mut PanelState) {
    let points: Vec<[f32; 2]> = if state.venue_editor.drawing {
        state.venue_editor.draft.clone()
    } else {
        state.venue_editor.edit_points.clone()
    };
    if points.len() < 2 {
        return;
    }
    let world: Vec<[f32; 3]> = match state.venue_editor.edit_basis {
        Some((origin, u, v)) if !state.venue_editor.drawing => points
            .iter()
            .map(|n| add3(origin, add3(scale3(u, n[0]), scale3(v, n[1]))))
            .collect(),
        _ => points.iter().map(|n| plane_point(*n)).collect(),
    };
    let (w, h) = extents(&world);
    let (w, h) = (format!("{w:.1}"), format!("{h:.1}"));
    ui.label(
        RichText::new(trf("{w} × {h} m", &[("w", &w), ("h", &h)]))
            .font(theme.font_small.clone())
            .color(theme.ink_weak),
    );
}

/// `POST /venue { op: "surface" }` — the one route this panel writes, with
/// `name` only when the surface is new so editing never renames it.
fn post_surface(door: &dyn Call, venue: &str, surface: &str, name: Option<&str>, points: &[[f32; 3]]) {
    let mut body = json!({ "op": "surface", "id": venue, "surface": surface, "points": points });
    if let Some(name) = name {
        body["name"] = json!(name);
    }
    let _ = door.json("POST", "/venue", Some(body));
}

/// The venue's surfaces out of a `GET /venue?id=` reply. The route's
/// serialised key has been seen both ways, so both are accepted.
fn surfaces_of(detail: &Value) -> Vec<Value> {
    detail["venue"]["surfaces"]
        .as_array()
        .or_else(|| detail["venue"]["surface"].as_array())
        .cloned()
        .unwrap_or_default()
}

/// A surface's `origin`/`u`/`v`, or `None` when any is missing.
fn basis_of(surface: &Value) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    Some((vec3(&surface["origin"])?, vec3(&surface["u"])?, vec3(&surface["v"])?))
}

fn vec3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32])
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// A fresh surface id no existing surface uses.
fn fresh_surface_id(surfaces: &[Value]) -> String {
    let mut n = surfaces.len() + 1;
    loop {
        let id = format!("surface-{n}");
        if !surfaces.iter().any(|s| s["id"] == json!(id)) {
            return id;
        }
        n += 1;
    }
}

/// The surface's own corners, the editor's fixed starting quad.
fn unit_quad() -> Vec<[f32; 2]> {
    vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
}

/// A new surface's corner on the ground plane: the canvas's `0..1`, Y-up,
/// centred on the origin and spanning [`WORLD_SPAN`] metres.
fn plane_point(n: [f32; 2]) -> [f32; 3] {
    [(n[0] - 0.5) * WORLD_SPAN, (0.5 - n[1]) * WORLD_SPAN, 0.0]
}

fn snap_all(points: Vec<[f32; 2]>, snap: bool) -> Vec<[f32; 2]> {
    if !snap {
        return points;
    }
    points
        .into_iter()
        .map(|p| {
            [
                (p[0] * SNAP_STEPS).round() / SNAP_STEPS,
                (p[1] * SNAP_STEPS).round() / SNAP_STEPS,
            ]
        })
        .collect()
}

/// The quad's width and height, in metres, over its corners.
fn extents(points: &[[f32; 3]]) -> (f32, f32) {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    for p in points {
        for axis in 0..2 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    ((max[0] - min[0]).abs(), (max[1] - min[1]).abs())
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use egui::Vec2;
    use egui_kittest::kittest::Queryable;
    use serde_json::json;

    use super::*;
    use crate::shell::FakeDoor;

    fn venue() -> Value {
        json!({
            "venue": {
                "venue": { "id": "bar", "name": "Bar", "setting": "indoor" },
                "fixture": [
                    { "id": "wash-1", "name": "wash 1", "kind": "wash", "profile": "example-wash",
                      "at": [2.5, 2.75, -2.4], "yaw": 0.0, "pitch": -20.0, "wire": "dmx" },
                    { "id": "wash-2", "name": "wash 2", "kind": "wash", "profile": "example-wash",
                      "at": [3.5, 2.75, -2.4], "yaw": 0.0, "pitch": -20.0, "wire": "dmx" }
                ]
            },
            "patch": [
                { "fixture": "wash-1", "kind": "wash", "wire": "dmx", "universe": 1, "address": 20, "span": 4 },
                { "fixture": "wash-2", "kind": "wash", "wire": "dmx", "universe": 1, "address": 22, "span": 4 }
            ]
        })
    }

    fn harness(door: Rc<FakeDoor>) -> egui_kittest::Harness<'static, State> {
        let theme = Theme::phosphor();
        let t = theme.clone();
        let h = egui_kittest::Harness::builder()
            .with_size(Vec2 { x: 720.0, y: 560.0 })
            .build_ui_state(move |ui, state: &mut State| section(ui, &t, door.as_ref(), "bar", "", &Value::Null, state), State::default());
        theme.apply(&h.ctx);
        h
    }

    /// M4.2: with a fixture selected, clicking the 3D view posts
    /// `POST /venue { op: "aim", fixture, target }`.
    #[test]
    fn aim_by_click_posts_aim() {
        let door = Rc::new(FakeDoor::new());
        door.respond("GET", "/venue", venue());
        door.respond("GET", "/venue/state", json!({ "fixtures": [] }));
        let mut h = harness(Rc::clone(&door));
        h.run();
        h.state_mut().selected = Some("wash-1".to_string());
        h.run();

        let p = h.state().view.expect("the 3D view painted a rect").center();
        h.drag_at(p);
        h.run();
        h.drop_at(p);
        h.run();

        let posted = door
            .calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == "/venue")
            .map(|(_, _, params)| serde_json::from_slice::<Value>(&params.body).unwrap())
            .find(|b| b["op"] == "aim")
            .expect("clicking the 3D view should POST /venue {op: aim}");
        assert_eq!(posted["fixture"], json!("wash-1"));
        assert!(posted["target"].as_array().is_some_and(|a| a.len() == 3), "{posted}");
    }

    /// M4.3: the patch view flags an address overlap — two fixtures on one
    /// wire and universe whose slot windows touch.
    #[test]
    fn the_patch_view_flags_an_address_overlap() {
        let read = json!({
            "patch": [
                { "fixture": "a", "wire": "dmx", "universe": 1, "address": 20, "span": 4 },
                { "fixture": "b", "wire": "dmx", "universe": 1, "address": 22, "span": 4 },
                { "fixture": "c", "wire": "dmx", "universe": 2, "address": 20, "span": 4 },
                { "fixture": "d", "wire": "usb", "universe": 0, "address": 0, "span": 0 }
            ]
        });
        let rows = patch_rows(&read);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].conflict && rows[1].conflict, "20..23 and 22..25 overlap");
        assert!(!rows[2].conflict, "a different universe is not an overlap");
        assert!(!rows[3].conflict, "a slotless fixture never overlaps");
        assert_eq!(rows[0].end(), 23);
    }
}
