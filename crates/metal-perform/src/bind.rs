//! The runtime of a binding (`metal_graph::bind`): every frame, each param edge
//! of the device graph reads its producer's value, maps it onto the
//! consumer's parameter, and writes it where that parameter lives **for this
//! frame** — never into a file (rule 4: the edge is the binding; this is only
//! its reading).
//!
//! # The mapping
//!
//! The producer's value is normalised by its variable's range to `0..1`
//! (clamped), then mapped onto the consumer parameter's `[min, max]`:
//!
//! ```text
//! t     = clamp((v - var.min) / (var.max - var.min), 0, 1)
//! value = param.min + t * (param.max - param.min)
//! ```
//!
//! So a `0..1` digest column swings a float across its **whole** declared
//! range (onset 1.0 → Turing's `escala` at 12), a script output declared
//! `0..2` reaches the top at 2, and nothing a binding does can leave a
//! parameter's range. A variable with an empty range maps to the parameter's
//! `min`.
//!
//! # Where a bound value lands
//!
//! - `gen.*`: the float's slot in `rows[gen]` — the row `Paint::run` draws.
//! - a common `fx.*`: the effect's slot in the effects block of every
//!   generator whose chain holds it (`row.len() - EFFECTS_WIDTH +
//!   effect_index`, the slot `crate::resolved_chain`/`apply_node_post` use),
//!   **and** the effect's `fixed` in the per-frame copy of the graph, which
//!   is what `resolved_chain` resolves a fixed amount from — a bound effect has
//!   no `control` cable (lint), so it reads as fixed at the bound value.
//! - `fx.estela`: the per-frame `fixed` (it has no row slot).
//! - `fx.text.*`: the per-frame `config.amount` the text pass reads.
//! - `script.*`: inside [`crate::script::evaluate_with`], as the float's value
//!   for this frame — the same override a wired `floats` port gives, one float
//!   by name instead of the row by position.
//!
//! # Held generator floats
//!
//! A generator float held in its node's config (`metal_graph::params`) is pinned
//! over the driven row before any binding, so a binding wins over it and an
//! unheld float keeps what the digest drive computed.
//!
//! # Order
//!
//! [`before_scripts`]: held floats, then every binding whose producer is not a
//! script, in plan order — so a script fed by a generator's `row` sees that
//! row's bindings. [`after_scripts`]: every binding again, in plan order —
//! the same value for the first ones, the final value for the script-fed ones.
//! One known limit: a script reading a generator row whose float is bound to
//! *another* script sees that float's unbound value this frame.
//!
//! # Live values
//!
//! [`Live`] keeps, per graph, what the last frame computed: every
//! parameter's effective value (`in`: bound, held or driven) and every
//! variable's value (`out`). `GET /graph/live` and `GET /graph/io` read it;
//! it costs one small map per rendered node per frame.

use std::collections::{BTreeMap, HashMap};
use web_time::Instant;

use serde_json::{json, Value};

use metal_graph::params::{self, Param, ParamKind};
use metal_graph::{Edge, Endpoint, Graph, NodeId, Plan};

/// Rows by node id — the generators' (`rows`) or the scripts' outputs.
pub type Rows = HashMap<NodeId, Vec<f32>>;

/// What the Looper is doing this frame, for the two `audio.*` variables that
/// are not digest columns (`repeat.locked`/`repeat.phase`). `Default` is the
/// no-loop reading (`locked` 0, `phase` 0), what a caller with no Looper in
/// hand — a test, a route — passes.
#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatVars {
    /// 1 while a loop is engaged, else 0.
    pub locked: f32,
    /// 0..1 through the loop (0 at the anchor beat).
    pub phase: f32,
}

/// Each generator node's parameters, from the painter's own schemas
/// ([`gen_params`]) — what a `row.<float>` variable and a `param:<float>` on a
/// generator resolve against at runtime.
pub type GenParams = HashMap<NodeId, Vec<Param>>;

/// The mapping of the module doc.
pub fn map_value(v: f32, from: (f32, f32), to: (f32, f32)) -> f32 {
    let span = from.1 - from.0;
    let t = if span.abs() <= f32::EPSILON || !v.is_finite() {
        0.0
    } else {
        ((v - from.0) / span).clamp(0.0, 1.0)
    };
    to.0 + t * (to.1 - to.0)
}

/// Every generator node of `graph` with its parameters, read off the
/// painter's schemas (`Paint::ids`/`Paint::schemas`, index for index). A node
/// naming a model the painter does not have is left out.
pub fn gen_params(graph: &Graph, ids: &[&str], schemas: &[&metal_schema::Schema]) -> GenParams {
    graph
        .nodes
        .iter()
        .filter_map(|n| {
            let model = n.kind.strip_prefix("gen.")?;
            let i = ids.iter().position(|id| *id == model)?;
            Some((n.id.clone(), params::schema_params(schemas.get(i)?)))
        })
        .collect()
}

/// The Looper value a name names among [`metal_graph::params::REPEAT_VARS`], or
/// `None` when it is not one of them (the caller then reads a digest column).
fn repeat_var(var: &str, repeat: &RepeatVars) -> Option<f32> {
    match var {
        "repeat.locked" => Some(repeat.locked),
        "repeat.phase" => Some(repeat.phase),
        _ => None,
    }
}

/// The value of the variable `from` names, this frame, with its range:
/// `(value, min, max)`. `None` when the producer has nothing this frame — a
/// generator off the composition, a script that errored or has not run —
/// and then the consumer keeps what it had (held or driven).
///
/// - `audio.*` `control.<column>`: the frame's digest column, `0..1`;
/// - `audio.*` `control.repeat.locked`/`.phase`: `repeat`, `0..1` — the two
///   Looper variables, appended past the sixteen digest columns (index 16, 17);
/// - `gen.*` `row.<float>`: the float in the generator's row, its own range;
/// - `script.*` `control.<output>`: the script's output (`scripts`), its
///   declared range or `0..1`;
/// - `sensor.*` `control.out<k>`: the frame's digest, the row a sensor's
///   `control` reads as today (`crate::script::evaluate`'s own reading).
pub fn var_value(
    graph: &Graph,
    from: &Endpoint,
    rows: &Rows,
    scripts: Option<&Rows>,
    gen_params: &GenParams,
    raw: &[f32],
    repeat: &RepeatVars,
) -> Option<(f32, f32, f32)> {
    let node = graph.node(&from.node)?;
    let (port, var) = params::split_var_port(&from.port)?;
    if node.kind.starts_with("audio.") && port == "control" {
        if let Some(v) = repeat_var(var, repeat) {
            return Some((v, 0.0, 1.0));
        }
        let i = metal_schema::DIGEST_COLUMNS.iter().position(|c| *c == var)?;
        return Some((*raw.get(i)?, 0.0, 1.0));
    }
    if node.kind.starts_with("gen.") && port == "row" {
        let p = gen_params.get(&node.id)?.iter().find(|p| p.name == var && p.kind == ParamKind::Float)?;
        let v = *rows.get(&node.id)?.get(p.index?)?;
        return Some((v, p.min, p.max));
    }
    // The kinds whose `control` row is computed live rather than stored: the
    // scripts and native presets, the Art-Net receiver, and the `source.*`
    // value nodes (M1).
    let live_row = metal_graph::is_script_kind(&node.kind)
        || node.kind == crate::artnet::ARTNET_IN_KIND
        || crate::source::is_source_kind(&node.kind);
    if live_row && port == "control" {
        let row = scripts?.get(&node.id)?;
        let v = params::node_vars(node, None, Some(row.len())).into_iter().find(|v| v.name == var)?;
        return Some((*row.get(v.index?)?, v.min, v.max));
    }
    if node.kind.starts_with("sensor.") && port == "control" {
        let v = params::node_vars(node, None, Some(raw.len())).into_iter().find(|v| v.name == var)?;
        return Some((*raw.get(v.index?)?, v.min, v.max));
    }
    None
}

/// The four colours a colour binding paints into a generator's colour slot —
/// the K mapping.
///
/// - a `color` node: its one colour fills **all four** stops, so a colour slot
///   driven by a single colour is a flat four-stop gradient of that colour (the
///   "fill the slot with this colour" gesture).
/// - a `palette` node: its own four slot colours fill the four stops in order —
///   the "palette as a whole" gesture, whatever single slot variable was bound.
///
/// `None` when the producer is neither kind, or (a palette) holds no colours:
/// then the consumer's slot is left alone rather than painted black.
pub fn colour_value(graph: &Graph, from: &Endpoint) -> Option<[[f32; 3]; 4]> {
    let node = graph.node(&from.node)?;
    if node.kind == metal_graph::COLOR_KIND {
        let config: Value = serde_json::from_str(&node.config.to_json_str()).ok()?;
        // The operator's picker writes `rgb` **and** its `h`/`s`/`b`; a script
        // or the digest drives `h`/`s`/`b` alone. When any of the three is
        // held, the colour is derived from them (the picker's own space), so a
        // driven hue actually repaints the slot. With none held, `rgb` is the
        // colour verbatim. Declared defaults: h 0, s 1, b 1.
        let held = |k: &str, d: f32| config.get(k).and_then(Value::as_f64).map(|v| v as f32).unwrap_or(d);
        let c = if ["h", "s", "b"].iter().any(|k| config.get(*k).is_some()) {
            metal_notation::color::Hsb::new(held("h", 0.0), held("s", 1.0), held("b", 1.0)).to_rgb()
        } else {
            let rgb = config.get("rgb")?.as_array()?;
            [
                rgb.first()?.as_f64()? as f32,
                rgb.get(1)?.as_f64()? as f32,
                rgb.get(2)?.as_f64()? as f32,
            ]
        };
        return Some([c; 4]);
    }
    if node.kind == metal_graph::PALETTE_KIND {
        let colors = metal_graph::params::palette_colors(node);
        if colors.is_empty() {
            return None;
        }
        // K: the palette's `shift` (a hue rotation, `-180..180`) moves the
        // whole set — a script or the digest can drive it. Rotating in OklCh
        // keeps lightness and chroma put, so "turn the palette" is only a turn.
        let config: Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
        let shift = config.get("shift").and_then(Value::as_f64).map(|v| v as f32).unwrap_or(0.0);
        let rotated: Vec<[f32; 3]> = if shift.abs() < f32::EPSILON {
            colors.clone()
        } else {
            colors.iter().map(|c| rotate_hue(*c, shift)).collect()
        };
        // Pad a short palette by repeating its last colour, so a two-colour
        // palette still fills all four stops rather than leaving two black.
        let mut out = [[0.0f32; 3]; 4];
        for i in 0..4 {
            out[i] = rotated.get(i).copied().unwrap_or(rotated[rotated.len() - 1]);
        }
        return Some(out);
    }
    None
}

/// Rotates an RGB triple's hue by `degrees` in OklCh, keeping lightness and
/// chroma — the palette `shift`'s reader. Back through `metal_notation`'s own
/// sRGB conversion, so a rotated colour is a displayable triple.
fn rotate_hue(rgb: [f32; 3], degrees: f32) -> [f32; 3] {
    use metal_notation::color::{to_srgb, Oklch};
    let c = oklch_of(rgb);
    let out = to_srgb(Oklch { h: c.h + degrees, ..c });
    [out.r as f32 / 255.0, out.g as f32 / 255.0, out.b as f32 / 255.0]
}

/// The OklCh of an RGB triple, reusing `metal_notation::color::Oklch` so there is
/// one colour space in the engine. `metal_notation` exposes `to_srgb` but not the
/// sRGB→OklCh direction, so it is done here from the same Ottosson matrix that
/// conversion uses.
fn oklch_of(rgb: [f32; 3]) -> metal_notation::color::Oklch {
    let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    let (r, g, b) = (lin(rgb[0].clamp(0.0, 1.0)), lin(rgb[1].clamp(0.0, 1.0)), lin(rgb[2].clamp(0.0, 1.0)));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    let l_ = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
    let a_ = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
    let b_ = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
    let c = (a_ * a_ + b_ * b_).sqrt();
    let h = if c <= f32::EPSILON { 0.0 } else { b_.atan2(a_).to_degrees().rem_euclid(360.0) };
    metal_notation::color::Oklch { l: l_, c, h }
}

/// Writes one number into a node's config in the per-frame graph — the copy
/// `render_device` loaded this frame, never the file.
pub(crate) fn set_config_number(graph: &mut Graph, node: &str, key: &str, v: f32) {
    let Some(n) = graph.node_mut(node) else { return };
    let mut config: Value = serde_json::from_str(&n.config.to_json_str()).unwrap_or(Value::Null);
    if !config.is_object() {
        config = json!({});
    }
    config[key] = json!(v);
    if let Ok(c) = metal_graph::Config::from_json_str(&config.to_string()) {
        n.config = c;
    }
}

/// Pins every held generator float over its driven row — see the module doc.
fn apply_held(graph: &Graph, rows: &mut Rows, gen_params: &GenParams) {
    for node in graph.nodes.iter().filter(|n| n.kind.starts_with("gen.")) {
        let held = params::gen_held(node);
        let vectors = params::gen_held_vecs(node);
        if held.is_empty() && vectors.is_empty() {
            continue;
        }
        let (Some(row), Some(ps)) = (rows.get_mut(&node.id), gen_params.get(&node.id)) else { continue };
        // A held colour slot or camera is written whole over the driven row.
        for p in ps.iter().filter(|p| p.kind != ParamKind::Float) {
            if let (Some(v), Some(at)) = (vectors.get(&p.name), p.index) {
                for (i, x) in v.iter().enumerate() {
                    if let Some(slot) = row.get_mut(at + i) {
                        *slot = *x;
                    }
                }
            }
        }
        for p in ps.iter().filter(|p| p.kind == ParamKind::Float) {
            if let (Some(v), Some(slot)) = (held.get(&p.name), p.index.and_then(|i| row.get_mut(i))) {
                *slot = v.clamp(p.min, p.max);
            }
        }
    }
}

/// The param edges this pass applies, in plan order of their consumer — a
/// generator bound from another generator's float is written after it.
/// Script consumers are [`crate::script::evaluate_with`]'s.
fn edges_in_order(graph: &Graph, plan: &Plan, with_scripts: bool) -> Vec<Edge> {
    let pos: HashMap<&str, usize> = plan.ops.iter().enumerate().map(|(i, op)| (op.node.as_str(), i)).collect();
    let is_script = |id: &str| graph.node(id).is_some_and(|n| metal_graph::is_script_kind(&n.kind));
    let mut edges: Vec<Edge> = graph
        .edges
        .iter()
        .filter(|e| params::is_param_edge(e) && !is_script(&e.to.node))
        .filter(|e| with_scripts || !is_script(&e.from.node))
        .cloned()
        .collect();
    edges.sort_by_key(|e| pos.get(e.to.node.as_str()).copied().unwrap_or(usize::MAX));
    edges
}

/// One binding: the producer's value, mapped, written where the consumer's
/// parameter lives this frame.
fn apply_edge(graph: &mut Graph, e: &Edge, rows: &mut Rows, scripts: Option<&Rows>, gen_params: &GenParams, raw: &[f32], repeat: &RepeatVars) {
    let Some(param) = params::param_name(&e.to.port) else { return };
    let Some(kind) = graph.node(&e.to.node).map(|n| n.kind.clone()) else { return };

    // K: a colour binding. The producer is a `color` or `palette` node, the
    // consumer's parameter is a colour slot (12 floats, four RGB stops). A
    // float value never reaches this arm — the producer's own variable kind is
    // checked by `metal_graph::bind` — but a colour var bound to a *float* param
    // would, and is skipped here (the lint refuses it before it renders).
    if kind.starts_with("gen.") {
        if let Some(p) = gen_params.get(&e.to.node).and_then(|ps| ps.iter().find(|p| p.name == param && p.kind == ParamKind::Colour)) {
            if let (Some(start), Some(colours)) = (p.index, colour_value(graph, &e.from)) {
                if let Some(row) = rows.get_mut(&e.to.node) {
                    for (i, c) in colours.iter().enumerate() {
                        let at = start + i * 3;
                        for (k, v) in c.iter().enumerate() {
                            if let Some(slot) = row.get_mut(at + k) {
                                *slot = v.clamp(0.0, 1.0);
                            }
                        }
                    }
                }
            }
            return;
        }
    }

    let Some((v, min, max)) = var_value(graph, &e.from, rows, scripts, gen_params, raw, repeat) else { return };
    // K: a `color`/`palette` node's own parameter is what a script or the
    // digest drives (`h`/`s`/`b`, or a palette's `shift`). It is held under
    // its own config key — the picker writes there too — and the colour the
    // node offers is then derived from it (`colour_value` below), so a driven
    // hue reaches every slot the node feeds. Ranges are the ones
    // `metal_graph::params::kind_params` declares.
    if kind == metal_graph::COLOR_KIND || kind == metal_graph::PALETTE_KIND {
        let range = match param {
            "h" => (0.0, 360.0),
            "shift" => (-180.0, 180.0),
            _ => (0.0, 1.0),
        };
        set_config_number(graph, &e.to.node, &param, map_value(v, (min, max), range));
        return;
    }
    // Parity P-A: `gen.ramp` is a source with no model row — its floats
    // live under their own config keys, like a multi-parameter effect's.
    if kind == metal_graph::RAMP_KIND {
        if let Some((_, lo, hi, _)) = metal_graph::parity::floats_of(&kind).and_then(|f| f.iter().find(|f| f.0 == param)) {
            set_config_number(graph, &e.to.node, &param, map_value(v, (min, max), (*lo, *hi)));
        }
        return;
    }
    if kind.starts_with("gen.") {
        let Some(p) = gen_params.get(&e.to.node).and_then(|ps| ps.iter().find(|p| p.name == param && p.kind == ParamKind::Float)) else {
            return;
        };
        let value = map_value(v, (min, max), (p.min, p.max));
        if let Some(slot) = p.index.and_then(|i| rows.get_mut(&e.to.node).and_then(|r| r.get_mut(i))) {
            *slot = value;
        }
    } else if kind.starts_with("fx.") {
        let Some(p) = params::effect_params(&kind).into_iter().find(|p| p.name == param) else { return };
        let value = map_value(v, (min, max), (p.min, p.max));
        if kind.starts_with("fx.text.") {
            set_config_number(graph, &e.to.node, "amount", value);
            return;
        }
        // K: `fx.move`'s parameters live under their own config keys — the
        // effect shader reads them there — so a binding writes the key, not
        // `fixed`.
        // T11: so do every multi-parameter effect's (`fx.particles`,
        // `fx.image-pop`, `fx.forge`, `fx.warp`).
        // Advanced: `fx.grade` and the six after it hold each parameter under
        // its own config key too (same shape as `fx.move`), so a binding into
        // `fx.grade`/`fx.texture`/… must write that name — `resolved_chain`
        // reads it from there.
        let advanced = kind
            .strip_prefix("fx.")
            .and_then(metal_schema::advanced_effect_floats)
            .is_some();
        if kind == crate::motion::FX_MOVE_KIND || params::effect_params(&kind).len() > 1 || advanced
        {
            set_config_number(graph, &e.to.node, &param, value);
            return;
        }
        set_config_number(graph, &e.to.node, "fixed", value);
        let Some(index) = kind.strip_prefix("fx.").and_then(metal_schema::effect_index) else { return };
        for gen in metal_graph::effects::generators_of(graph, &e.to.node) {
            if let Some(row) = rows.get_mut(&gen) {
                if row.len() >= metal_schema::EFFECTS_WIDTH {
                    let at = row.len() - metal_schema::EFFECTS_WIDTH + index;
                    row[at] = value;
                }
            }
        }
    }
    // Extension point: a later consumer kind (`color`, `palette`, …) lands here.
}

/// The first pass of a frame, before the scripts run: held generator floats,
/// then every binding whose producer is not a script. `graph` is the
/// per-frame copy; `plan` its compiled plan (the order).
pub fn before_scripts(graph: &mut Graph, plan: &Plan, rows: &mut Rows, gen_params: &GenParams, raw: &[f32], repeat: &RepeatVars) {
    apply_held(graph, rows, gen_params);
    for e in edges_in_order(graph, plan, false) {
        apply_edge(graph, &e, rows, None, gen_params, raw, repeat);
    }
}

/// The second pass, once the scripts have answered: every binding, final.
pub fn after_scripts(
    graph: &mut Graph,
    plan: &Plan,
    rows: &mut Rows,
    scripts: &Rows,
    gen_params: &GenParams,
    raw: &[f32],
    repeat: &RepeatVars,
) {
    for e in edges_in_order(graph, plan, true) {
        apply_edge(graph, &e, rows, Some(scripts), gen_params, raw, repeat);
    }
}

/// The amount an `fx.*` node applies this frame, by the rule
/// `crate::resolved_chain` resolves it with: fixed → its (per-frame, possibly
/// bound) fixed value; reactive from raw ingest → the effect's digest column;
/// reactive from a row → the generator's slot; switched off → 0. A text
/// effect reads its `amount` (default 1). `None` where no amount applies (a
/// reactive `wake`, `fx.forge`).
fn effective_amount(graph: &Graph, fx: &str, rows: &Rows, raw: &[f32]) -> Option<f32> {
    let node = graph.node(fx)?;
    if node.kind.starts_with("fx.text.") {
        let config: Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or(Value::Null);
        return Some(config.get("amount").and_then(Value::as_f64).map(|a| a as f32).unwrap_or(1.0));
    }
    let name = node.kind.strip_prefix("fx.")?;
    let index = metal_schema::effect_index(name);
    if index.is_none() && metal_schema::footage_chain_kind(name).is_none() {
        return None;
    }
    match metal_graph::effects::state_of(graph, fx) {
        metal_schema::EffectState::Fixed(v) => Some(v),
        metal_schema::EffectState::Off => index.map(|_| 0.0),
        metal_schema::EffectState::Reactive => {
            let index = index?;
            match metal_graph::effects::control_source(graph, fx) {
                Some(metal_graph::effects::ControlSource::RawIngest) => {
                    let spec = &metal_schema::EFFECTS_FLOATS[index];
                    let col = *metal_schema::effect_column_map().get(index)?;
                    raw.get(col).map(|v| v.clamp(spec.min, spec.max))
                }
                _ => metal_graph::effects::generators_of(graph, fx).iter().find_map(|g| {
                    let row = rows.get(g)?;
                    row.get(row.len().checked_sub(metal_schema::EFFECTS_WIDTH)? + index).copied()
                }),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// live values
// ---------------------------------------------------------------------------

/// One node's values in the last frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveNode {
    /// Every parameter's effective value — bound, held or driven.
    pub inputs: Vec<(String, f32)>,
    /// Every variable's value.
    pub outputs: Vec<(String, f32)>,
    /// How many values a live-sized `control` (a script) answered.
    pub width: Option<usize>,
}

/// One graph's last frame.
#[derive(Debug, Clone)]
pub struct LiveGraph {
    /// The show clock (`State::time`) the frame was drawn at.
    pub t: f32,
    /// When it was recorded, for a reader to tell a stale graph (no longer
    /// rendered) from a running one.
    pub at: Instant,
    pub nodes: BTreeMap<NodeId, LiveNode>,
}

impl LiveGraph {
    /// `{ "t", "age_ms", "nodes": { <node>: { "in": {param: v}, "out": {var: v} } } }`.
    pub fn to_json(&self) -> Value {
        let map = |pairs: &[(String, f32)]| -> Value {
            Value::Object(pairs.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
        };
        let nodes: serde_json::Map<String, Value> = self
            .nodes
            .iter()
            .map(|(id, n)| (id.clone(), json!({ "in": map(&n.inputs), "out": map(&n.outputs) })))
            .collect();
        json!({ "t": self.t, "age_ms": self.at.elapsed().as_millis() as u64, "nodes": nodes })
    }
}

impl metal_graph::bind::LiveValues for LiveGraph {
    fn width(&self, node: &str) -> Option<usize> {
        self.nodes.get(node)?.width
    }
    fn input(&self, node: &str, param: &str) -> Option<f32> {
        self.nodes.get(node)?.inputs.iter().find(|(k, _)| k == param).map(|(_, v)| *v)
    }
    fn output(&self, node: &str, var: &str) -> Option<f32> {
        self.nodes.get(node)?.outputs.iter().find(|(k, _)| k == var).map(|(_, v)| *v)
    }
}

/// The last frame of every graph rendered, by graph id — `State::live`.
#[derive(Debug, Default)]
pub struct Live {
    graphs: HashMap<String, LiveGraph>,
}

impl Live {
    /// The last frame recorded for graph `id`, if it has been rendered.
    pub fn graph(&self, id: &str) -> Option<&LiveGraph> {
        self.graphs.get(id)
    }

    /// Records one frame of `graph`: after both passes and the scripts, so
    /// every value is the one this frame draws with. `script_floats` is what
    /// each script was handed ([`crate::script::Evaluated::floats`]).
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        graph: &Graph,
        rows: &Rows,
        scripts: &Rows,
        script_floats: &Rows,
        gen_params: &GenParams,
        raw: &[f32],
        t: f32,
    ) {
        let mut nodes = BTreeMap::new();
        for node in &graph.nodes {
            let mut live = LiveNode::default();
            if node.kind.starts_with("gen.") {
                let (Some(row), Some(ps)) = (rows.get(&node.id), gen_params.get(&node.id)) else { continue };
                for p in ps.iter().filter(|p| p.kind == ParamKind::Float) {
                    if let Some(v) = p.index.and_then(|i| row.get(i)) {
                        live.inputs.push((p.name.clone(), *v));
                        live.outputs.push((p.name.clone(), *v));
                    }
                }
            } else if node.kind.starts_with("fx.") {
                let Some(v) = effective_amount(graph, &node.id, rows, raw) else { continue };
                live.inputs.push(("amount".to_string(), v));
            } else if metal_graph::is_script_kind(&node.kind) {
                let floats = params::node_params(node, None);
                if let Some(values) = script_floats.get(&node.id) {
                    live.inputs = floats.iter().zip(values).map(|(p, v)| (p.name.clone(), *v)).collect();
                }
                if let Some(row) = scripts.get(&node.id) {
                    live.width = Some(row.len());
                    live.outputs = params::node_vars(node, None, Some(row.len()))
                        .into_iter()
                        .filter_map(|v| Some((v.name, *row.get(v.index?)?)))
                        .collect();
                }
            } else if crate::source::is_source_kind(&node.kind) {
                // M1: a `source.*` node's held floats are its inputs; its
                // computed row is its outputs (`out0..`).
                let ps = params::node_params(node, None);
                for p in ps.iter().filter(|p| p.kind == ParamKind::Float) {
                    if let Some(v) = p.value {
                        live.inputs.push((p.name.clone(), v));
                    }
                }
                if let Some(row) = scripts.get(&node.id) {
                    live.width = Some(row.len());
                    live.outputs = params::node_vars(node, None, Some(row.len()))
                        .into_iter()
                        .filter_map(|v| Some((v.name, *row.get(v.index?)?)))
                        .collect();
                }
            } else if node.kind.starts_with("audio.") || node.kind.starts_with("sensor.") {
                live.outputs = params::node_vars(node, None, Some(raw.len()))
                    .into_iter()
                    .filter_map(|v| Some((v.name, *raw.get(v.index?)?)))
                    .collect();
            } else {
                continue;
            }
            nodes.insert(node.id.clone(), live);
        }
        self.graphs.insert(graph.id.clone(), LiveGraph { t, at: Instant::now(), nodes });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metal_graph::{Config, Domain};

    fn manifest() -> metal_schema::Manifest {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).unwrap()).unwrap()
    }

    /// The painter's view of the suite, as `render_device` hands it over.
    fn suite() -> (Vec<&'static str>, Vec<&'static metal_schema::Schema>) {
        let suite = manifest().into_suite();
        let ids = suite.iter().map(|e| &*Box::leak(e.id.clone().into_boxed_str())).collect();
        let schemas = suite.iter().map(|e| e.schema).collect();
        (ids, schemas)
    }

    fn turing_row(ids: &[&str], schemas: &[&'static metal_schema::Schema]) -> Vec<f32> {
        schemas[ids.iter().position(|id| *id == "turing").unwrap()].defaults()
    }

    fn param<'a>(gp: &'a GenParams, node: &str, name: &str) -> &'a Param {
        gp[node].iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn the_mapping_spans_the_consumers_range() {
        assert_eq!(map_value(0.0, (0.0, 1.0), (1.0, 12.0)), 1.0, "the bottom of the variable is the parameter's min");
        assert_eq!(map_value(1.0, (0.0, 1.0), (1.0, 12.0)), 12.0, "the top reaches its max");
        assert_eq!(map_value(0.5, (0.0, 1.0), (-1.0, 1.0)), 0.0);
        assert_eq!(map_value(1.0, (0.0, 2.0), (0.0, 1.0)), 0.5, "normalised by the variable's own range");
        assert_eq!(map_value(9.0, (0.0, 1.0), (0.0, 1.0)), 1.0, "never out of the parameter's range");
        assert_eq!(map_value(-3.0, (0.0, 1.0), (0.2, 0.8)), 0.2);
        assert_eq!(map_value(0.7, (0.5, 0.5), (0.2, 0.8)), 0.2, "an empty variable range maps to min");
        assert_eq!(map_value(f32::NAN, (0.0, 1.0), (0.2, 0.8)), 0.2);
    }

    /// audio.live `control.rms` → `fx.vignette`'s amount: the vignette slot of
    /// the generator's row and the effect's per-frame fixed value both follow
    /// the digest column, so `resolved_chain` draws it.
    #[test]
    fn a_digest_column_bound_to_an_effect_amount_drives_it() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
        g.node_mut(&fx).unwrap().config = metal_graph::effects::FxConfig { fixed: 0.1 }.to_config();
        g.add_edge(Endpoint::new(&audio, "control.rms"), Endpoint::new(&fx, "param:amount"));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        let mut raw = [0.0f32; 16];
        raw[0] = 0.8;

        before_scripts(&mut g, &plan, &mut rows, &gp, &raw, &RepeatVars::default());

        let at = rows[&gen].len() - metal_schema::EFFECTS_WIDTH + metal_schema::effect_index("vignette").unwrap();
        assert!((rows[&gen][at] - 0.8).abs() < 1e-6, "the row slot follows rms");
        assert_eq!(metal_graph::effects::state_of(&g, &fx), metal_schema::EffectState::Fixed(0.8));
        let mut post = metal_schema::read_post(&rows[&gen]);
        let chain = crate::resolved_chain(&g, &gen, &mut post, &raw);
        assert_eq!(chain.len(), 1);
        assert!((chain[0].amount - 0.8).abs() < 1e-6, "the drawn amount is the bound one");
    }

    /// `audio.live` `control.repeat.phase` → `fx.vignette`'s amount: the two
    /// Looper variables are not digest columns, so the value comes from the
    /// `RepeatVars` the pass is handed, not from `raw`.
    #[test]
    fn repeat_locked_and_phase_drive_a_bound_parameter() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let fx = g.add_node("fx", "fx.vignette", [2.0, 0.0]);
        let out = g.add_node("out", "output.device", [3.0, 0.0]);
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&out, "image"));
        g.node_mut(&fx).unwrap().config = metal_graph::effects::FxConfig { fixed: 0.1 }.to_config();
        g.add_edge(Endpoint::new(&audio, "control.repeat.phase"), Endpoint::new(&fx, "param:amount"));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        let raw = [0.0f32; 16];
        let rv = RepeatVars { locked: 1.0, phase: 0.5 };

        before_scripts(&mut g, &plan, &mut rows, &gp, &raw, &rv);

        let at = rows[&gen].len() - metal_schema::EFFECTS_WIDTH + metal_schema::effect_index("vignette").unwrap();
        assert!((rows[&gen][at] - 0.5).abs() < 1e-6, "the row slot follows phase, not a digest column");
        assert_eq!(metal_graph::effects::state_of(&g, &fx), metal_schema::EffectState::Fixed(0.5));
    }

    /// While a loop is engaged the Looper's `locked` reads 1 through
    /// `control.repeat.locked`; `RepeatVars::default()` (no loop) reads 0.
    #[test]
    fn a_bound_repeat_locked_reads_one_while_engaged() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control.repeat.locked"), Endpoint::new(&gen, "param:feed"));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let feed = param(&gp, &gen, "feed");

        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars { locked: 1.0, phase: 0.5 });
        assert_eq!(rows[&gen][feed.index.unwrap()], feed.max, "engaged: locked is 1");

        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars::default());
        assert_eq!(rows[&gen][feed.index.unwrap()], feed.min, "no loop: locked is 0");
    }

    /// A script's output bound to a generator float overrides what the drive
    /// computed for it, mapped onto the float's range; the other floats keep
    /// their driven value.
    #[test]
    fn a_script_output_bound_to_a_generator_float_overrides_the_driven_row() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&script, "control.out0"), Endpoint::new(&gen, "param:scale"));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let driven = turing_row(&ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), driven.clone())]);
        let scripts = Rows::from([(script.clone(), vec![0.5])]);

        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars::default());
        assert_eq!(rows[&gen], driven, "a script's value only lands once it has answered");
        after_scripts(&mut g, &plan, &mut rows, &scripts, &gp, &[], &RepeatVars::default());

        let scale = param(&gp, &gen, "scale");
        let want = scale.min + 0.5 * (scale.max - scale.min);
        assert!((rows[&gen][scale.index.unwrap()] - want).abs() < 1e-5);
        let other = param(&gp, &gen, "feed").index.unwrap();
        assert_eq!(rows[&gen][other], driven[other], "the rest of the row is untouched");
    }

    #[test]
    fn a_held_generator_float_is_pinned_and_a_binding_wins_over_it() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        g.node_mut(&gen).unwrap().config =
            Config::from_json_str(r#"{"floats": {"scale": 3.0, "feed": 0.02}}"#).unwrap();
        g.add_edge(Endpoint::new(&audio, "control.onset"), Endpoint::new(&gen, "param:feed"));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        let mut raw = [0.0f32; 16];
        raw[2] = 1.0;
        before_scripts(&mut g, &plan, &mut rows, &gp, &raw, &RepeatVars::default());
        assert_eq!(rows[&gen][param(&gp, &gen, "scale").index.unwrap()], 3.0, "held");
        let feed = param(&gp, &gen, "feed");
        assert_eq!(rows[&gen][feed.index.unwrap()], feed.max, "bound beats held");
    }

    /// A held colour slot and a held camera are written whole over the driven
    /// row; the rest of the row is left alone.
    #[test]
    fn a_held_colour_and_camera_are_pinned_whole() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let ps = gp[&gen].clone();
        let colour = ps.iter().find(|p| p.kind == ParamKind::Colour).expect("turing has a colour slot").clone();
        let camera = ps.iter().find(|p| p.kind == ParamKind::Camera).cloned();
        let mut held = serde_json::json!({ "colours": { colour.name.clone(): vec![0.5f32; 12] } });
        if camera.is_some() {
            held["camera"] = serde_json::json!([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        }
        g.node_mut(&gen).unwrap().config = Config::from_json_str(&held.to_string()).unwrap();
        let driven = turing_row(&ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), driven.clone())]);
        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars::default());
        let at = colour.index.unwrap();
        assert!(rows[&gen][at..at + 12].iter().all(|v| *v == 0.5), "the colour slot is the held one");
        if let Some(c) = camera {
            let at = c.index.unwrap();
            assert_eq!(rows[&gen][at..at + 7], [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        }
        let other = param(&gp, &gen, "feed").index.unwrap();
        assert_eq!(rows[&gen][other], driven[other], "the rest of the row is untouched");
    }

    /// K: a `color` node bound to a generator's colour slot paints all four
    /// stops with its colour — the row slot is where the renderer reads them.
    #[test]
    fn a_colour_node_fills_a_generators_colour_slot() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let colour = g.add_node("color", metal_graph::COLOR_KIND, [0.0, 1.0]);
        g.node_mut(&colour).unwrap().config =
            Config::from_json_str(r#"{"rgb":[1.0,0.0,0.0]}"#).unwrap();
        let slot = gen_params(&g, &ids, &schemas)[&gen]
            .iter()
            .find(|p| p.kind == ParamKind::Colour)
            .unwrap()
            .clone();
        let from = params::var_port("control", "rgb");
        g.add_edge(Endpoint::new(&colour, &from), Endpoint::new(&gen, params::param_port(&slot.name)));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);

        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars::default());

        let at = slot.index.unwrap();
        for stop in 0..4 {
            assert_eq!(&rows[&gen][at + stop * 3..at + stop * 3 + 3], &[1.0, 0.0, 0.0],
                "stop {stop} of the colour slot follows the bound colour");
        }
    }

    /// K: a `palette` node's four slots fill the four stops, whatever single
    /// slot variable was bound — the "palette as a whole" gesture.
    #[test]
    fn a_palette_node_fills_the_four_stops_with_its_slots() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("g", "t", Domain::Live);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let palette = g.add_node("palette", metal_graph::PALETTE_KIND, [0.0, 1.0]);
        g.node_mut(&palette).unwrap().config = Config::from_json_str(
            r#"{"colors":[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[1.0,1.0,0.0]]}"#,
        )
        .unwrap();
        let slot = gen_params(&g, &ids, &schemas)[&gen]
            .iter()
            .find(|p| p.kind == ParamKind::Colour)
            .unwrap()
            .clone();
        let from = params::var_port("control", "base");
        g.add_edge(Endpoint::new(&palette, &from), Endpoint::new(&gen, params::param_port(&slot.name)));
        let plan = g.compile();
        let gp = gen_params(&g, &ids, &schemas);
        let mut rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);

        before_scripts(&mut g, &plan, &mut rows, &gp, &[], &RepeatVars::default());

        let at = slot.index.unwrap();
        assert_eq!(&rows[&gen][at..at + 3], &[1.0, 0.0, 0.0]);
        assert_eq!(&rows[&gen][at + 3..at + 6], &[0.0, 1.0, 0.0]);
        assert_eq!(&rows[&gen][at + 6..at + 9], &[0.0, 0.0, 1.0]);
        assert_eq!(&rows[&gen][at + 9..at + 12], &[1.0, 1.0, 0.0]);
    }

    /// K: a `color` whose HSB is held (a picker write, or a script driving
    /// `h`/`s`/`b`) offers the colour those numbers name — not the stale
    /// `rgb` beside them. Driving `h` alone repaints the slot.
    #[test]
    fn a_colours_hsb_drives_the_colour_it_offers() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let colour = g.add_node("color", metal_graph::COLOR_KIND, [0.0, 0.0]);
        // `rgb` says red, but the held HSB says a pure green.
        g.node_mut(&colour).unwrap().config =
            Config::from_json_str(r#"{"rgb":[1.0,0.0,0.0],"h":120.0,"s":1.0,"b":1.0}"#).unwrap();
        let from = params::var_port("control", "rgb");
        let c = colour_value(&g, &Endpoint::new(&colour, &from)).unwrap();
        for stop in c {
            assert!((stop[1] - 1.0).abs() < 1e-5 && stop[0] < 1e-5, "HSB must win over rgb: {stop:?}");
        }

        // With no HSB held, `rgb` is the colour verbatim.
        let plain = g.add_node("color", metal_graph::COLOR_KIND, [0.0, 1.0]);
        g.node_mut(&plain).unwrap().config = Config::from_json_str(r#"{"rgb":[0.0,0.0,1.0]}"#).unwrap();
        let c = colour_value(&g, &Endpoint::new(&plain, &from)).unwrap();
        assert_eq!(c[0], [0.0, 0.0, 1.0]);
    }

    /// K: a palette's `shift` rotates every slot's hue and leaves lightness and
    /// chroma — the whole set turns, so the palette stays a palette.
    #[test]
    fn a_palettes_shift_rotates_its_colours() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let palette = g.add_node("palette", metal_graph::PALETTE_KIND, [0.0, 0.0]);
        g.node_mut(&palette).unwrap().config = Config::from_json_str(
            r#"{"colors":[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[1.0,1.0,0.0]]}"#,
        )
        .unwrap();
        let from = params::var_port("control", "base");
        let plain = colour_value(&g, &Endpoint::new(&palette, &from)).unwrap();

        // A 120° rotation of red is green.
        g.node_mut(&palette).unwrap().config = Config::from_json_str(
            r#"{"colors":[[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0],[1.0,1.0,0.0]],"shift":120.0}"#,
        )
        .unwrap();
        let shifted = colour_value(&g, &Endpoint::new(&palette, &from)).unwrap();
        assert_ne!(plain[0], shifted[0], "the shift must move the slot");
        assert!(shifted[0][1] > shifted[0][0], "red rotated 120° reads green-ish: {:?}", shifted[0]);
        // Every slot moved — the whole palette, not one colour.
        for (a, b) in plain.iter().zip(&shifted) {
            assert_ne!(a, b, "every slot turns together");
        }
    }

    #[test]
    fn live_records_what_each_node_used_and_offered() {
        let (ids, schemas) = suite();
        let mut g = Graph::new("device-x", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [1.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [2.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 1.0]);
        g.node_mut(&script).unwrap().config = Config::from_json_str(
            r#"{"code": "", "floats": [{"name": "gain", "min": 0, "max": 1, "default": 0.5}], "outputs": ["x"]}"#,
        )
        .unwrap();
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.node_mut(&fx).unwrap().config = metal_graph::effects::FxConfig { fixed: 0.3 }.to_config();
        let gp = gen_params(&g, &ids, &schemas);
        let rows = Rows::from([(gen.clone(), turing_row(&ids, &schemas))]);
        let scripts = Rows::from([(script.clone(), vec![0.25, 0.75])]);
        let floats = Rows::from([(script.clone(), vec![0.5])]);
        let mut raw = [0.0f32; 16];
        raw[2] = 0.9;

        let mut live = Live::default();
        live.record(&g, &rows, &scripts, &floats, &gp, &raw, 12.5);
        let frame = live.graph("device-x").expect("recorded under the graph's id");
        use metal_graph::bind::LiveValues;
        assert_eq!(frame.output(&audio, "onset"), Some(0.9));
        assert_eq!(frame.input(&fx, "amount"), Some(0.3), "fixed: the held amount");
        assert_eq!(frame.input(&script, "gain"), Some(0.5));
        assert_eq!((frame.output(&script, "x"), frame.output(&script, "out1")), (Some(0.25), Some(0.75)));
        assert_eq!(frame.width(&script), Some(2));
        let scale = param(&gp, &gen, "scale");
        assert_eq!(frame.input(&gen, "scale"), Some(scale.default));
        let json = frame.to_json();
        assert_eq!(json["t"], json!(12.5));
        assert_eq!(json["nodes"][&fx]["in"]["amount"], json!(0.3_f32));
        assert!(live.graph("elsewhere").is_none());
    }
}
