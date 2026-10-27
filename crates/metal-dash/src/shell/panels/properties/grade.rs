//! The colour grade: the `fx.grade` node's whole Lightroom-style surface, plus
//! the grouped slider page the six other advanced effects (`fx.texture`,
//! `fx.clarity`, `fx.dehaze`, `fx.denoise`, `fx.optics`, `fx.geometry`, `fx.fm`) get on
//! their `effect` tab.
//!
//! **Rule 5.** Every range, name and default comes off `GET /graph/io`'s own
//! `params` list (which `metal-graph` builds from `metal_schema`'s declared floats);
//! every current value comes off the node's `config` in the `GET /graph` the
//! browser already holds. Nothing here re-derives a parameter. The `/graph/io`
//! reply is the browser's cached one, handed in — never fetched per frame.
//!
//! **One writer.** Each edit posts `POST /graph/node {op:"config",
//! config:{<name>: <value>}}` — the same merge `fx.move` uses. While a handle
//! is held the widget keeps its own value and this page posts at most every
//! [`LIVE_POST_GAP`] (each post saves the graph), then once more on release,
//! so the picture follows the hand without a write per frame.
//!
//! **Structure** (2026-10-09 rework): Basic (white balance · tone · presence),
//! Tone Curve, Color Mixer (one property across the eight bands, the way a
//! grader compares them), Color Grading (three wheels side by side, global
//! under them). Each section folds and has its own reset; the page header
//! resets everything.

use crate::shell::components::small_text;
use egui::{Id, Ui};
use serde_json::{json, Map, Value};

use crate::call::Call;
use crate::shell::classes::Theme;
use crate::shell::components::{
    curve_graph_fixed, grading_wheel_sized, histogram, hue_band_selector, kv_wrap, note, section as kit_section, segmented,
    slider_tracked, Track,
};
use crate::shell::i18n::{tr, tr_key};

use super::shared::post;

/// The eight colour-mixer bands, in wheel order — their config-key stems and
/// their `tr_key`-marked display names, one list so the two cannot drift.
const BAND_KEYS: [&str; 8] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];
const BAND_LABELS: [&str; 8] = [
    tr_key("red"),
    tr_key("orange"),
    tr_key("yellow"),
    tr_key("green"),
    tr_key("aqua"),
    tr_key("blue"),
    tr_key("purple"),
    tr_key("magenta"),
];

/// The least time between two live posts of one gesture, in seconds.
const LIVE_POST_GAP: f64 = 0.08;

/// The rows of the Basic section, grouped, with their display names.
const WHITE_BALANCE: [(&str, &str); 2] = [
    ("grade_temperature", tr_key("Temp")),
    ("grade_tint", tr_key("Tint")),
];
const TONE: [(&str, &str); 6] = [
    ("grade_exposure", tr_key("Exposure")),
    ("grade_contrast", tr_key("Contrast")),
    ("grade_highlights", tr_key("Highlights")),
    ("grade_shadows", tr_key("Shadows")),
    ("grade_whites", tr_key("Whites")),
    ("grade_blacks", tr_key("Blacks")),
];
const PRESENCE: [(&str, &str); 2] = [
    ("grade_vibrance", tr_key("Vibrance")),
    ("grade_saturation", tr_key("Saturation")),
];

/// The four parametric-curve floats, in shadow-to-highlight order, with the
/// curve's fixed `x` for each and their display names.
const CURVE_FLOATS: [&str; 4] = [
    "grade_curve_shadows",
    "grade_curve_darks",
    "grade_curve_lights",
    "grade_curve_highlights",
];
const CURVE_X: [f32; 4] = [0.2, 0.4, 0.6, 0.8];
const CURVE_LABELS: [&str; 4] = [tr_key("Shadows"), tr_key("Darks"), tr_key("Lights"), tr_key("Highlights")];
const CURVE_SPLITS: [(&str, &str); 4] = [
    ("grade_split_shadows", tr_key("Shadow split")),
    ("grade_split_mid", tr_key("Midtone split")),
    ("grade_split_highlights", tr_key("Highlight split")),
    ("grade_refine_saturation", tr_key("Refine saturation")),
];

/// The grading zones, left to right, then the global wheel.
const ZONES: [(&str, &str); 3] = [
    ("shadows", tr_key("shadows")),
    ("midtones", tr_key("midtones")),
    ("highlights", tr_key("highlights")),
];

/// One parameter's declared span off `/graph/io`, plus the node's held values
/// off `/graph` — the whole read this page draws from.
struct Params {
    spans: std::collections::HashMap<String, (f32, f32, f32)>,
    config: Map<String, Value>,
}

impl Params {
    fn read(io: Option<&Value>, graph: &Value, node_id: &str) -> Params {
        let mut spans = std::collections::HashMap::new();
        if let Some(list) = io.and_then(|v| v["params"].as_array()) {
            for p in list {
                if let Some(name) = p["name"].as_str() {
                    spans.insert(
                        name.to_string(),
                        (
                            p["min"].as_f64().unwrap_or(-100.0) as f32,
                            p["max"].as_f64().unwrap_or(100.0) as f32,
                            p["default"].as_f64().unwrap_or(0.0) as f32,
                        ),
                    );
                }
            }
        }
        let config = graph["graph"]["nodes"]
            .as_array()
            .and_then(|ns| ns.iter().find(|n| n["id"] == json!(node_id)))
            .and_then(|n| n.get("config"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Params { spans, config }
    }

    /// `(min, max, default)` for `name`, the grade's own bipolar `-100..100`
    /// when `/graph/io` did not answer.
    fn span(&self, name: &str) -> (f32, f32, f32) {
        self.spans.get(name).copied().unwrap_or((-100.0, 100.0, 0.0))
    }

    /// The held value of `name`, its declared default when unheld.
    fn value(&self, name: &str) -> f32 {
        self.config
            .get(name)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .unwrap_or(self.span(name).2)
    }

    /// Whether `name` sits off its default.
    fn edited(&self, name: &str) -> bool {
        (self.value(name) - self.span(name).2).abs() > f32::EPSILON
    }
}

/// Where this page writes: the door and the node, plus the throttle key.
struct Target<'a> {
    door: &'a dyn Call,
    gid: &'a str,
    node: &'a str,
}

impl Target<'_> {
    /// `POST /graph/node {op:"config"}` — the one write this page makes.
    /// `live` (a handle still held) is throttled to [`LIVE_POST_GAP`]; a
    /// release always posts.
    fn post(&self, ui: &Ui, fields: Map<String, Value>, live: bool) {
        if fields.is_empty() {
            return;
        }
        let key = Id::new(("shell/region/properties/grade/last-post", self.gid, self.node));
        let now = ui.input(|i| i.time);
        if live {
            let last = ui.data(|d| d.get_temp::<f64>(key)).unwrap_or(f64::NEG_INFINITY);
            if now - last < LIVE_POST_GAP {
                crate::shell::frames::repaint(ui.ctx());
                return;
            }
        }
        ui.data_mut(|d| d.insert_temp(key, now));
        post(
            self.door,
            "/graph/node",
            json!({ "op": "config", "graph": self.gid, "node": self.node, "config": Value::Object(fields) }),
        );
    }

    /// Every `names` back to its declared default, in one merge.
    fn reset(&self, ui: &Ui, p: &Params, names: impl IntoIterator<Item = String>) {
        let mut fields = Map::new();
        for name in names {
            if p.edited(&name) {
                let default = p.span(&name).2;
                fields.insert(name, json!(default));
            }
        }
        self.post(ui, fields, false);
    }
}

/// One tracked slider row for `name`, labelled `label`.
fn row(ui: &mut Ui, theme: &Theme, t: &Target, p: &Params, name: &str, label: &str, track: &Track) {
    let (min, max, default) = p.span(name);
    let mut value = p.value(name);
    let response = slider_tracked(ui, theme, label, &mut value, min..=max, default, track, true);
    if response.changed() {
        let mut fields = Map::new();
        fields.insert(name.to_string(), json!(value));
        t.post(ui, fields, response.is_pointer_button_down_on());
    }
}

/// A small weak-ink sub-heading inside a section.
fn subhead(ui: &mut Ui, theme: &Theme, text: &str) {
    ui.add_space(theme.item_gap.y);
    ui.label(small_text(theme, text).color(theme.ink_weak).strong());
}

/// A folding section whose open state lives in egui memory (so a click
/// sticks), with a reset at the header's end when any of `names` is edited.
/// Returns whether the body should paint.
fn fold(ui: &mut Ui, theme: &Theme, t: &Target, p: &Params, title: &str, names: &[String], default_open: bool) -> bool {
    let key = Id::new(("shell/region/properties/grade/open", title));
    let mut open = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(default_open);
    let edited = names.iter().any(|n| p.edited(n));
    ui.horizontal(|ui| {
        kit_section(ui, theme, title, &mut open);
        if edited {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button(small_text(theme, tr("reset")))
                    .on_hover_text(tr("Back to the defaults"))
                    .clicked()
                {
                    t.reset(ui, p, names.iter().cloned());
                }
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(key, open));
    open
}

fn names<'a>(list: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    list.into_iter().map(str::to_string).collect()
}

/// The `fx.grade` page. Returns `true` when it painted (always, for
/// `fx.grade`). `io` is the browser's cached `/graph/io` reply.
#[allow(clippy::too_many_arguments)]
pub(crate) fn section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    kind: &str,
    gid: &str,
    node_id: &str,
    graph: &Value,
    io: Option<&Value>,
) -> bool {
    if kind != "fx.grade" {
        return false;
    }
    let p = Params::read(io, graph, node_id);
    let t = Target { door, gid, node: node_id };
    let all: Vec<String> = p.spans.keys().filter(|k| k.starts_with("grade_")).cloned().collect();
    let edited = all.iter().filter(|n| p.edited(n)).count();

    note(
        ui,
        theme,
        &if edited == 0 {
            tr("no adjustments").to_string()
        } else {
            format!("{edited} {}", tr("adjustments"))
        },
    );
    if edited > 0 {
        kv_wrap(ui, theme, "", |ui| {
            if ui.small_button(small_text(theme, tr("reset all"))).clicked() {
                t.reset(ui, &p, all.iter().cloned());
            }
        });
    }
    note(ui, theme, tr("drag · shift = fine · double-click = default · wheel nudges"));

    // The focused grade's own stage, histogrammed by the canvas preview
    // that pumps it (`preview::publish_histogram`) — shown only once a
    // frame has landed, never as an empty box.
    if let Some(h) = crate::shell::preview::histogram(ui.ctx()) {
        let _ = histogram(ui, theme, &h[0], &h[1], &h[2], &h[3]);
    }

    // --- Basic --------------------------------------------------------------
    let basic = names(WHITE_BALANCE.iter().chain(&TONE).chain(&PRESENCE).map(|(n, _)| *n));
    if fold(ui, theme, &t, &p, tr("Basic"), &basic, true) {
        subhead(ui, theme, tr("White Balance"));
        row(ui, theme, &t, &p, WHITE_BALANCE[0].0, tr(WHITE_BALANCE[0].1), &Track::Temp);
        row(ui, theme, &t, &p, WHITE_BALANCE[1].0, tr(WHITE_BALANCE[1].1), &Track::Tint);
        subhead(ui, theme, tr("Tone"));
        for (name, label) in TONE {
            row(ui, theme, &t, &p, name, tr(label), &Track::Centered);
        }
        subhead(ui, theme, tr("Presence"));
        row(
            ui,
            theme,
            &t,
            &p,
            PRESENCE[0].0,
            tr(PRESENCE[0].1),
            &Track::Gradient { from: "grey".to_string(), to: "orange".to_string() },
        );
        row(
            ui,
            theme,
            &t,
            &p,
            PRESENCE[1].0,
            tr(PRESENCE[1].1),
            &Track::Gradient { from: "grey".to_string(), to: "magenta".to_string() },
        );
    }

    // --- Tone curve ---------------------------------------------------------
    let curve = names(CURVE_FLOATS.iter().copied().chain(CURVE_SPLITS.iter().map(|(n, _)| *n)));
    if fold(ui, theme, &t, &p, tr("Tone Curve"), &curve, false) {
        let mut points = curve_points(&p);
        let response = curve_graph_fixed(ui, theme, &mut points, "master");
        if response.changed() {
            curve_commit(ui, &t, &p, &points, response.is_pointer_button_down_on());
        }
        for k in 0..4 {
            row(ui, theme, &t, &p, CURVE_FLOATS[k], tr(CURVE_LABELS[k]), &Track::Centered);
        }
        subhead(ui, theme, tr("Regions"));
        for (name, label) in CURVE_SPLITS {
            row(ui, theme, &t, &p, name, tr(label), &Track::Plain);
        }
    }

    // --- Color mixer --------------------------------------------------------
    let mixer_names: Vec<String> = BAND_KEYS
        .iter()
        .flat_map(|b| ["hue", "sat", "lum"].map(|s| format!("grade_mixer_{b}_{s}")))
        .collect();
    if fold(ui, theme, &t, &p, tr("Color Mixer"), &mixer_names, false) {
        mixer(ui, theme, &t, &p);
    }

    // --- Color grading ------------------------------------------------------
    let grading_names: Vec<String> = ["shadows", "midtones", "highlights", "global"]
        .iter()
        .flat_map(|z| ["hue", "sat", "lum"].map(|s| format!("grade_grading_{z}_{s}")))
        .chain(["grade_blending".to_string(), "grade_balance".to_string()])
        .collect();
    if fold(ui, theme, &t, &p, tr("Color Grading"), &grading_names, false) {
        color_grading(ui, theme, &t, &p);
    }
    true
}

/// The tone curve through the four parametric floats: the diagonal's
/// endpoints and one point per float, its `-100..100` mapped to a `±0.5` lift
/// above the diagonal at that `x`.
fn curve_points(p: &Params) -> Vec<[f32; 2]> {
    let mut points = vec![[0.0, 0.0]];
    for k in 0..4 {
        let lift = p.value(CURVE_FLOATS[k]).clamp(-100.0, 100.0) / 200.0;
        points.push([CURVE_X[k], (CURVE_X[k] + lift).clamp(0.0, 1.0)]);
    }
    points.push([1.0, 1.0]);
    points
}

/// The fixed curve's handles back to their floats — handle `k + 1` is float
/// `k`, its lift above the diagonal scaled back to `-100..100`.
fn curve_commit(ui: &Ui, t: &Target, p: &Params, points: &[[f32; 2]], live: bool) {
    let mut fields = Map::new();
    for k in 0..4 {
        let Some(point) = points.get(k + 1) else { continue };
        let (min, max, _) = p.span(CURVE_FLOATS[k]);
        let value = ((point[1] - CURVE_X[k]) * 200.0).clamp(min, max);
        if (value - p.value(CURVE_FLOATS[k])).abs() > 0.5 {
            fields.insert(CURVE_FLOATS[k].to_string(), json!(value));
        }
    }
    t.post(ui, fields, live);
}

/// The HSL mixer: pick a property (hue / saturation / luminance) and see it
/// across all eight bands at once — or pick `band` to see one colour's three.
fn mixer(ui: &mut Ui, theme: &Theme, t: &Target, p: &Params) {
    let mode_key = Id::new("shell/region/properties/grade/mixer-mode");
    let band_key = Id::new("shell/region/properties/grade/band");
    let mut mode = ui.data(|d| d.get_temp::<usize>(mode_key)).unwrap_or(0);
    let options = [(0usize, tr("Hue")), (1, tr("Saturation")), (2, tr("Luminance")), (3, tr("band"))];
    if segmented(ui, theme, &mut mode, &options).changed() {
        ui.data_mut(|d| d.insert_temp(mode_key, mode));
    }
    if mode < 3 {
        let suffix = ["hue", "sat", "lum"][mode];
        for (band, key) in BAND_KEYS.iter().enumerate() {
            let track = match mode {
                0 => Track::Hue { band },
                1 => Track::Sat { band },
                _ => Track::Lum { band },
            };
            let name = format!("grade_mixer_{key}_{suffix}");
            row(ui, theme, t, p, &name, tr(BAND_LABELS[band]), &track);
        }
        return;
    }
    let mut band = ui.data(|d| d.get_temp::<usize>(band_key)).unwrap_or(0);
    if let Some(picked) = hue_band_selector(ui, theme, band) {
        band = picked;
        ui.data_mut(|d| d.insert_temp(band_key, band));
    }
    subhead(ui, theme, tr(BAND_LABELS[band]));
    for (suffix, label, track) in [
        ("hue", tr("Hue"), Track::Hue { band }),
        ("sat", tr("Saturation"), Track::Sat { band }),
        ("lum", tr("Luminance"), Track::Lum { band }),
    ] {
        let name = format!("grade_mixer_{}_{suffix}", BAND_KEYS[band]);
        row(ui, theme, t, p, &name, label, &track);
    }
}

/// The three zone wheels side by side, each with its luminance under it,
/// then the global wheel and the blending/balance pair.
fn color_grading(ui: &mut Ui, theme: &Theme, t: &Target, p: &Params) {
    let gap = theme.item_gap.x;
    let col = ((ui.available_width() - gap * 2.0) / 3.0).max(theme.wheel_radius);
    let radius = (col * 0.5 - theme.wheel_handle).min(theme.wheel_radius).max(theme.wheel_handle * 4.0);
    ui.columns(3, |cols| {
        for (i, (zone, label)) in ZONES.iter().enumerate() {
            zone_wheel(&mut cols[i], theme, t, p, zone, tr(label), radius);
        }
    });
    ui.add_space(theme.item_gap.y);
    zone_wheel(ui, theme, t, p, "global", tr("global"), radius);
    row(ui, theme, t, p, "grade_blending", tr("Blending"), &Track::Plain);
    row(ui, theme, t, p, "grade_balance", tr("Balance"), &Track::Centered);
}

/// One zone: the hue/sat wheel bound to `<zone>_hue`/`_sat` (both in one
/// merge), its luminance as a compact track under it.
#[allow(clippy::too_many_arguments)]
fn zone_wheel(ui: &mut Ui, theme: &Theme, t: &Target, p: &Params, zone: &str, label: &str, radius: f32) {
    let hue_name = format!("grade_grading_{zone}_hue");
    let sat_name = format!("grade_grading_{zone}_sat");
    let lum_name = format!("grade_grading_{zone}_lum");
    let mut hue = p.value(&hue_name);
    let mut sat = p.value(&sat_name);
    let response = grading_wheel_sized(ui, theme, &mut hue, &mut sat, label, radius);
    if response.changed() {
        let mut fields = Map::new();
        fields.insert(hue_name, json!(hue));
        fields.insert(sat_name, json!(sat));
        t.post(ui, fields, response.is_pointer_button_down_on());
    }
    let (min, max, default) = p.span(&lum_name);
    let mut lum = p.value(&lum_name);
    let r = slider_tracked(ui, theme, tr("Lum"), &mut lum, min..=max, default, &Track::Centered, true);
    if r.changed() {
        let mut fields = Map::new();
        fields.insert(lum_name, json!(lum));
        t.post(ui, fields, r.is_pointer_button_down_on());
    }
}

/// The `effect` tab for an advanced effect: one section of tracked sliders, its
/// ranges straight off `/graph/io`. Returns `true` when `kind` is one of the
/// advanced effects (and `false` so the caller falls back to [`super::effects`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn advanced_section(
    ui: &mut Ui,
    theme: &Theme,
    door: &dyn Call,
    kind: &str,
    gid: &str,
    node_id: &str,
    graph: &Value,
    io: Option<&Value>,
) -> bool {
    let Some(name) = kind.strip_prefix("fx.") else {
        return false;
    };
    if !metal_schema::ADVANCED_EFFECT_IDS.contains(&name) {
        return false;
    }
    let p = Params::read(io, graph, node_id);
    let t = Target { door, gid, node: node_id };
    let mut open = true;
    kit_section(ui, theme, effect_title(name), &mut open);
    if let Some(list) = io.and_then(|v| v["params"].as_array()) {
        for entry in list {
            let Some(param) = entry["name"].as_str() else { continue };
            if entry["kind"] != json!("float") {
                continue;
            }
            let min = entry["min"].as_f64().unwrap_or(-100.0) as f32;
            row(ui, theme, &t, &p, param, param, &advanced_track(name, min));
        }
    }
    true
}

/// The track an advanced effect's row wears: the local-contrast trio and
/// geometry are bipolar; denoise and the positive-only optics dials are plain.
fn advanced_track(name: &str, min: f32) -> Track {
    match name {
        "texture" | "clarity" | "dehaze" | "geometry" => Track::Centered,
        "denoise" => Track::Plain,
        _ => {
            if min < 0.0 {
                Track::Centered
            } else {
                Track::Plain
            }
        }
    }
}

/// The `tr_key`-marked title an advanced effect's section wears.
fn effect_title(name: &str) -> &'static str {
    match name {
        "grade" => tr("grade"),
        "texture" => tr("texture"),
        "clarity" => tr("clarity"),
        "dehaze" => tr("dehaze"),
        "denoise" => tr("denoise"),
        "optics" => tr("optics"),
        "geometry" => tr("geometry"),
        "fm" => tr("fm"),
        "wavewarp" => tr("wavewarp"),
        "ascii" => tr("ascii"),
        "brush" => tr("brush"),
        "hardstutter" => tr("hardstutter"),
        _ => tr("effect"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_round_trips_its_four_floats() {
        let graph = json!({ "graph": { "nodes": [ { "id": "g", "config": {
            "grade_curve_shadows": 40.0, "grade_curve_darks": -20.0,
            "grade_curve_lights": 0.0, "grade_curve_highlights": 100.0 } } ] } });
        let p = Params::read(None, &graph, "g");
        let pts = curve_points(&p);
        for k in 0..4 {
            let back = (pts[k + 1][1] - CURVE_X[k]) * 200.0;
            assert!((back - p.value(CURVE_FLOATS[k])).abs() < 0.5 || pts[k + 1][1] >= 1.0);
        }
    }
}
