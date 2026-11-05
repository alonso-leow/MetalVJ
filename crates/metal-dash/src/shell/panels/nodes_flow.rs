//! Flow states on the node canvas: which nodes are live (an AI or AI FX
//! generator that is not bypassed), which just changed, and the animated
//! cables that show both. The canvas calls [`observe`] once a frame; the
//! cable and point painters read the result back from the egui context, so
//! `nodes.rs` carries only the hook calls.

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use egui::{Color32, Id, Painter, Pos2, Stroke};
use serde_json::Value;

use crate::shell::classes::Theme;

/// Samples per cable curve: enough for the bands to read as moving, few
/// enough that a graph of cables stays cheap to paint.
const SEGMENTS: usize = 24;

/// Share of each band's period that is lit; the rest is the transparent gap.
const BAND_DUTY: f32 = 0.6;

/// Seconds a changed node's fade-out takes at the end of its window.
const FADE_SECS: f64 = 0.3;

/// What one frame knows about the graph's flow, built by [`observe`].
#[derive(Default)]
struct Frame {
    now: f64,
    /// Live nodes: AI or AI FX generators that are not bypassed.
    live: HashSet<String>,
    /// Nodes that changed recently and are not bypassed, with their strength
    /// (1 at the change, fading over the last [`FADE_SECS`] of the window).
    flowing: HashMap<String, f32>,
    /// Bypassed nodes: their input cable is drawn dim.
    bypassed: HashSet<String>,
    /// Graph positions of the live nodes, to find their compacted points.
    live_pos: Vec<[f32; 2]>,
}

/// What survives between frames: each node's last fingerprint and the time
/// it last changed.
#[derive(Clone, Default)]
struct Book {
    prints: HashMap<String, u64>,
    changed: HashMap<String, f64>,
}

fn frame_id() -> Id {
    Id::new("metal-dash/nodes-flow/frame")
}

fn book_id() -> Id {
    Id::new("metal-dash/nodes-flow/book")
}

fn read(ctx: &egui::Context) -> Option<Arc<Frame>> {
    ctx.data(|d| d.get_temp::<Arc<Frame>>(frame_id()))
}

/// Builds this frame's flow from the graph and its live values. A node's
/// fingerprint is its record without `pos` (a drag must not count as a change)
/// plus its live inputs, so an overlay, a properties edit or a bound float
/// all register the same way. Asks for repaints while anything still moves.
pub fn observe(ctx: &egui::Context, theme: &Theme, graph: Option<&Value>, live: Option<&Value>) {
    let now = ctx.input(|i| i.time);
    let window = theme.flow_window_secs();
    let mut book: Book = ctx.data(|d| d.get_temp(book_id())).unwrap_or_default();
    let mut frame = Frame { now, ..Default::default() };
    let nodes = graph.and_then(|g| g["graph"]["nodes"].as_array()).map(Vec::as_slice).unwrap_or_default();
    for n in nodes {
        let Some(id) = n["id"].as_str() else { continue };
        let bypassed = n["bypass"].as_bool().unwrap_or(false);
        if bypassed {
            frame.bypassed.insert(id.to_string());
        }
        let mut body = n.clone();
        if let Some(o) = body.as_object_mut() {
            o.remove("pos");
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        body.to_string().hash(&mut hasher);
        if let Some(inputs) = live.map(|l| &l["nodes"][id]["in"]) {
            inputs.to_string().hash(&mut hasher);
        }
        let print = hasher.finish();
        // A node seen for the first time has no earlier value to differ from.
        if book.prints.insert(id.to_string(), print).is_some_and(|old| old != print) {
            book.changed.insert(id.to_string(), now);
        }
        // A generator in AI or AI FX mode (the default when `mode` is unset)
        // is live; `manual` holds its row and sends nothing on its own.
        let kind = n["kind"].as_str().unwrap_or_default();
        let mode = n["config"]["mode"].as_str().unwrap_or("ai");
        if kind.starts_with("gen.") && mode != "manual" && !bypassed {
            frame.live.insert(id.to_string());
            if let (Some(x), Some(y)) = (n["pos"][0].as_f64(), n["pos"][1].as_f64()) {
                frame.live_pos.push([x as f32, y as f32]);
            }
        }
        let age = book.changed.get(id).map(|t| now - t);
        if let (Some(age), false) = (age, bypassed) {
            if age < window {
                let ramp = ((window - age) / FADE_SECS).min(1.0) as f32;
                frame.flowing.insert(id.to_string(), ramp);
            }
        }
    }
    // A headless harness steps in coarse quarter-second frames; the loop would
    // never settle there, so only a real frame clock keeps the flow moving.
    let real_clock = ctx.input(|i| i.predicted_dt) < 0.1;
    if real_clock && (!frame.live.is_empty() || !frame.flowing.is_empty()) {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    ctx.data_mut(|d| {
        d.insert_temp(book_id(), book);
        d.insert_temp(frame_id(), Arc::new(frame));
    });
}

/// The two node ids a cable joins, output end first.
fn ends(p: &Value) -> [Option<&str>; 2] {
    [p["from"]["node"].as_str(), p["to"]["node"].as_str()]
}

/// A cable's stroke: the theme's own, dimmed when the node it feeds is
/// bypassed (the input side carries nothing).
pub fn cable_stroke(ctx: &egui::Context, theme: &Theme, p: &Value, accent: bool) -> Stroke {
    let base = theme.node_cable_stroke(accent);
    let Some(f) = read(ctx) else { return base };
    match p["to"]["node"].as_str() {
        Some(to) if f.bypassed.contains(to) => {
            Stroke::new(base.width, base.color.gamma_multiply(theme.flow_dim()))
        }
        _ => base,
    }
}

/// The flow over one cable, drawn on top of it: moving bands with a wide
/// faint aura, a band of colour and a thin glassy core. It runs when either
/// end is a live node (full strength) or a node that just changed (fading).
pub fn paint_cable_flow(ctx: &egui::Context, painter: &Painter, theme: &Theme, curve: [Pos2; 4], p: &Value) {
    let Some(f) = read(ctx) else { return };
    let strength = ends(p).into_iter().flatten().fold(0.0f32, |s, id| {
        let live = if f.live.contains(id) { 1.0 } else { 0.0 };
        s.max(live).max(f.flowing.get(id).copied().unwrap_or(0.0))
    });
    if strength <= 0.0 {
        return;
    }
    let pts = sample(curve);
    let t = f.now as f32;
    let colours = theme.flow_colours();
    let width = theme.node_cable_width;
    let bands = theme.flow_bands();
    let aura_phase = (t / theme.flow_aura_secs()).fract();
    let band_phase = (t / theme.flow_band_secs()).fract();
    band_pass(painter, &pts, aura_phase, bands, &colours, width * 4.0, 0.22 * strength);
    band_pass(painter, &pts, band_phase, bands, &colours, width * 2.0, 0.9 * strength);
    band_pass(painter, &pts, band_phase, bands, &[Color32::WHITE], width * 0.6, 0.55 * strength);
}

/// One layer of moving bands: a segment is lit where its place along the
/// curve falls in a band, the band's colour cycles through `palette`, and the
/// gaps between bands stay clear. `phase` in 0..1 moves the bands left to right.
fn band_pass(
    painter: &Painter,
    pts: &[Pos2],
    phase: f32,
    bands: f32,
    palette: &[Color32],
    width: f32,
    alpha: f32,
) {
    let last = (pts.len() - 1) as f32;
    for (i, w) in pts.windows(2).enumerate() {
        let u = ((i as f32 + 0.5) / last) * bands - phase;
        let inside = u.rem_euclid(1.0);
        if inside > BAND_DUTY {
            continue;
        }
        // Smooth across the band: zero at both edges, full in the middle.
        let lit = (inside / BAND_DUTY * PI).sin() * alpha;
        let cell = u.floor() as i64;
        let colour = palette[cell.rem_euclid(palette.len() as i64) as usize];
        painter.line_segment([w[0], w[1]], Stroke::new(width, colour.gamma_multiply(lit)));
    }
}

/// The points of a cubic bezier, `SEGMENTS + 1` of them, in screen space.
fn sample(c: [Pos2; 4]) -> Vec<Pos2> {
    (0..=SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32;
            let m = 1.0 - t;
            let v = c[0].to_vec2() * (m * m * m)
                + c[1].to_vec2() * (3.0 * m * m * t)
                + c[2].to_vec2() * (3.0 * m * t * t)
                + c[3].to_vec2() * (t * t * t);
            v.to_pos2()
        })
        .collect()
}

/// The beat of a live node's glow, 0..1, about one pulse every 1.6 s.
fn beat(now: f64) -> f32 {
    (now as f32 * 2.0 * PI / 1.6).sin() * 0.5 + 0.5
}

/// A compacted (point) live node pulses blue. Called after the point is drawn.
pub fn paint_point_pulse(ctx: &egui::Context, painter: &Painter, theme: &Theme, centre: Pos2, p: &Value) {
    let Some(f) = read(ctx) else { return };
    let at = [p["x"].as_f64().unwrap_or(0.0) as f32, p["y"].as_f64().unwrap_or(0.0) as f32];
    if !f.live_pos.contains(&at) {
        return;
    }
    let b = beat(f.now);
    let blue = theme.flow_colours()[0];
    let r = theme.node_point_radius;
    painter.circle_filled(centre, r * (2.0 + 1.2 * b), blue.gamma_multiply(0.15 + 0.2 * b));
    painter.circle_filled(centre, r, blue);
}

/// A live node's parameter point gets a pulsing cyan ring, the highlight that
/// marks a generator whose values are driven by the AI. Called after the dot.
pub fn paint_param_glow(ctx: &egui::Context, painter: &Painter, theme: &Theme, centre: Pos2, node: &str, radius: f32) {
    let Some(f) = read(ctx) else { return };
    if !f.live.contains(node) {
        return;
    }
    let b = beat(f.now);
    let cyan = theme.flow_colours()[1];
    painter.circle_stroke(
        centre,
        radius * (1.6 + 0.6 * b),
        Stroke::new(theme.hairline, cyan.gamma_multiply(0.4 + 0.5 * b)),
    );
}
