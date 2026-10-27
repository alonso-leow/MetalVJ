//! H5: `sensor.yolo`'s page — the camera it watches, a live preview with the
//! detected head dot and face box, the One Euro smoothing dials, and a "lost
//! track" indicator.
//!
//! **The camera is the upstream `capture`'s, not a second setting.** A
//! `sensor.yolo` node has one `image` input, and the picker writes the
//! `source` on the `capture` node that feeds it (rule 4: config is a reading
//! of the graph). The choices come off `GET /io`, the same list the capture
//! page uses.
//!
//! **The preview and the indicator are reads, not copies.** The picture pumps
//! `GET /sensor.jpg` (the capture frame with the head drawn on it) and the
//! indicator reads `GET /sensor`'s one fact — the reading the engine holds
//! this frame, stale or not. Neither is kept here between frames.

use crate::shell::components::small_text;
use egui::{RichText, Ui};
use serde_json::{json, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{kv_row, note, num_value};
use crate::shell::frames::{live_gap, Frame};
use crate::shell::i18n::tr;

use super::shared::get;
use super::sources::{io_listing, pick_row};
use super::text::post_text;

/// The One Euro dials' ranges. `min_cutoff` is Hz (lower smooths harder),
/// `beta` is the speed gain; both are the plan's `min_cutoff`/`beta`.
const MIN_CUTOFF_RANGE: std::ops::RangeInclusive<f32> = 0.1..=20.0;
const BETA_RANGE: std::ops::RangeInclusive<f32> = 0.0..=5.0;
const DEFAULT_MIN_CUTOFF: f32 = 1.0;
const DEFAULT_BETA: f32 = 0.5;


/// What this page keeps between frames: the preview pump.
#[derive(Default)]
pub struct State {
    pub frame: Frame,
}

/// The `capture` node feeding `node_id` through its `image` port, if any.
fn upstream_capture(graph: &Value, node_id: &str) -> Option<String> {
    graph["graph"]["edges"]
        .as_array()?
        .iter()
        .find(|e| e["to"]["node"] == json!(node_id) && e["to"]["port"] == json!("image"))
        .and_then(|e| e["from"]["node"].as_str())
        .map(str::to_string)
}

/// The `source` written on a node in the graph reply.
fn node_source(graph: &Value, id: &str) -> String {
    graph["graph"]["nodes"]
        .as_array()
        .and_then(|ns| ns.iter().find(|n| n["id"] == json!(id)))
        .and_then(|n| n["config"]["source"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The head dot and the face box, drawn over the picture (G6: the window
/// draws them, the JPEG carries only the capture). The box grows with
/// `eye_z_est` (closer is bigger) and takes the lost-track colour while stale.
fn draw_head(ui: &Ui, theme: &Theme, rect: egui::Rect, eye: (f32, f32, f32), stale: bool) {
    let (ex, ey, ez) = eye;
    let at = rect.min + egui::Vec2::new(ex.clamp(0.0, 1.0) * rect.width(), ey.clamp(0.0, 1.0) * rect.height());
    let half = (rect.width() * theme.head_box_scale * (theme.head_box_near + ez.clamp(0.0, 1.0))).max(theme.head_box_min);
    let painter = ui.painter_at(rect);
    let mut stroke = theme.rule_bright_stroke();
    stroke.color = if stale { theme.warn } else { theme.accent_vivid };
    painter.rect_stroke(egui::Rect::from_center_size(at, egui::Vec2::splat(half) * 2.0), theme.square_corner(), stroke, egui::StrokeKind::Outside);
    painter.circle_filled(at, theme.head_dot_radius, theme.ink);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    gid: &str,
    node_id: &str,
    node: &Value,
    graph: &Value,
    state: &mut State,
) {
    // --- the camera (written on the upstream capture) --------------------
    let capture = upstream_capture(graph, node_id);
    let current = capture.as_deref().map(|c| node_source(graph, c)).unwrap_or_default();
    let io = io_listing(ui, door, false);
    let choices: Vec<(String, String)> = io["video"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| Some((s["id"].as_str()?.to_string(), s["label"].as_str()?.to_string())))
        .collect();
    if let Some(pick) = pick_row(ui, theme, "sensor-camera", tr("camera"), &current, &choices) {
        match &capture {
            Some(c) => post_text(door, gid, c, json!({ "source": pick })),
            None => note(ui, theme, tr("wire a `capture` node into this sensor's image input first.")),
        }
    }
    if capture.is_none() {
        note(ui, theme, tr("no capture upstream: this sensor reads nothing yet."));
    }

    // --- live preview with the head dot and face box ---------------------
    ui.separator();
    let width = ui.available_width().min(360.0);
    let height = (width * 9.0 / 16.0).max(60.0);
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(width, height), egui::Sense::hover());
    let query = vec![
        ("graph", gid.to_string()),
        ("node", node_id.to_string()),
        ("w", (width.round() as u32).to_string()),
        ("h", (height.round() as u32).to_string()),
    ];
    let full = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0));
    match state.frame.picture(ui.ctx(), door, "/sensor.jpg", &query, live_gap()) {
        Some((id, _)) => ui.painter().image(id, rect, full, theme.image_tint),
        None => ui.painter().rect_filled(rect, theme.corner_radius(), theme.tube),
    };

    // --- the lost-track indicator ----------------------------------------
    let reading = get(
        door,
        "GET",
        "/sensor",
        &[("graph", gid.to_string()), ("node", node_id.to_string())],
    )
    .unwrap_or_default();
    let has = reading["reading"].as_bool().unwrap_or(false);
    let stale = reading["stale"].as_bool().unwrap_or(true);
    if has {
        let x = reading["eye_x"].as_f64().unwrap_or(0.5) as f32;
        let y = reading["eye_y"].as_f64().unwrap_or(0.5) as f32;
        let z = reading["eye_z_est"].as_f64().unwrap_or(0.5) as f32;
        draw_head(ui, theme, rect, (x, y, z), stale);
    }
    kv_row(ui, theme, tr("track"), |ui| {
        let (text, colour) = if !has {
            (tr("— no reading yet —"), theme.ink_faint)
        } else if stale {
            (tr("lost track"), theme.warn)
        } else {
            (tr("following head"), theme.accent_vivid)
        };
        let mut r = ui.label(small_text(theme, text).color(colour));
        if has {
            let x = reading["eye_x"].as_f64().unwrap_or(0.5);
            let y = reading["eye_y"].as_f64().unwrap_or(0.5);
            let z = reading["eye_z_est"].as_f64().unwrap_or(0.5);
            r = ui.label(
                RichText::new(format!("{x:.2}, {y:.2} · {z:.2}"))
                    .font(theme.font_small.clone())
                    .color(theme.ink_faint),
            );
        }
        r
    });

    // --- the One Euro smoothing dials ------------------------------------
    ui.separator();
    let mut min_cutoff = node["config"]["min_cutoff"].as_f64().unwrap_or(DEFAULT_MIN_CUTOFF as f64) as f32;
    kv_row(ui, theme, tr("min cutoff"), |ui| {
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut min_cutoff, MIN_CUTOFF_RANGE, "", 2, DEFAULT_MIN_CUTOFF, true);
        if r.changed() {
            post_text(door, gid, node_id, json!({ "min_cutoff": min_cutoff }));
        }
        r
    });
    let mut beta = node["config"]["beta"].as_f64().unwrap_or(DEFAULT_BETA as f64) as f32;
    kv_row(ui, theme, tr("beta"), |ui| {
        let w = ui.available_width();
        let r = num_value(ui, theme, w, &mut beta, BETA_RANGE, "", 2, DEFAULT_BETA, true);
        if r.changed() {
            post_text(door, gid, node_id, json!({ "beta": beta }));
        }
        r
    });
    note(ui, theme, tr("lower cutoff smooths a still head harder; higher beta follows a fast one."));
}
