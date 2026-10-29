//! Node parameters and variables: what a node can be **driven by** and what it
//! **offers downstream** — the "Parameters" and "Source" lists of the
//! properties panel, and the rows a Resolume-Wire-style canvas draws with an
//! input dot each.
//!
//! # Two lists, both readings
//!
//! - A [`Param`] is an input a binding can drive: a generator's own floats
//!   (and its colour slots), an effect's `amount`, a script's declared floats.
//!   Its `value` is what the node **holds** in its config while nothing drives
//!   it.
//! - A [`Var`] is what a node offers on one of its output ports: the sixteen
//!   digest columns of an audio node's `control`, a generator's picture and the
//!   floats of its `row`, a script's outputs.
//!
//! Neither list is stored anywhere (rule 4). The static half — a generator's
//! floats, an effect's amount — is stamped on the catalog entry
//! ([`crate::NodeKind::params`]) by [`crate::NodeKind::catalog`] from the
//! manifest (rule 7: no hand-written list), so anything holding the catalog
//! (lint, layout, a route) can read it without a manifest in hand. The
//! per-node half — a script's floats and outputs, every held value — is read
//! off `Node.config` the moment it is asked for.
//!
//! # The port syntax of a binding
//!
//! A binding is an ordinary [`crate::Edge`] (see [`crate::bind`]) from one
//! variable of an upstream output port to one parameter of the consumer:
//!
//! - `from.port` = `<output port>.<var name>` — `control.rms`, `row.scale`,
//!   `control.out0`. Port names never carry a `.`; var names may carry spaces
//!   or `#` (`control.chroma C#`), so the split is at the **first** `.` only
//!   ([`split_var_port`]).
//! - `to.port` = `param:<param name>` ([`param_port`]).
//!
//! # Where a held value lives
//!
//! One place per kind, each one the value the node already carried or the one
//! its existing writer already wrote:
//!
//! - `fx.*` (a common effect, `wake`; `quimica` was retired 2026-09-28):
//!   `FxConfig::fixed`, the "fixed" value of [`crate::effects`].
//! - `fx.text.*`: `config.amount`, what `metal_perform`'s text pass reads.
//! - `script.*`: `config.floats[i].value` ([`crate::script_floats`]).
//! - `gen.*`: `config.floats` as an **object** `{"<float>": value}` — the key
//!   `set_params {floats: {...}}` and `POST /graph/node {op: "config"}` already
//!   wrote on a generator (nothing read it until now). A held generator float
//!   is *fixed* the way a fijo effect is: the runtime pins it over the driven
//!   row, a binding wins over it, and clearing it (`null`) hands the float back
//!   to the digest drive.
//!
//! Five models name a colour slot like one of their own floats (`escape`,
//! `neon`, `iris`, `bola`, `plomo`): `param:<name>` always means the float.
//! Colour bindings are the extension point below; when they land they need a
//! port spelling of their own.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{Config, Edge, Graph, Node, NodeId, NodeKind, PortType};

// ---------------------------------------------------------------------------
// the port syntax
// ---------------------------------------------------------------------------

/// The prefix every parameter port carries: `param:amount`.
pub const PARAM_PREFIX: &str = "param:";

/// The key a generator node holds its fixed floats under — see the module doc.
pub const GEN_HELD_KEY: &str = "floats";

/// `param:<name>`, the `to.port` of a binding.
pub fn param_port(name: &str) -> String {
    format!("{PARAM_PREFIX}{name}")
}

/// The parameter a port names (`param:amount` → `amount`), or `None` for an
/// ordinary port.
pub fn param_name(port: &str) -> Option<&str> {
    port.strip_prefix(PARAM_PREFIX).filter(|n| !n.is_empty())
}

/// Whether `edge` is a binding (a param edge) rather than an image or control
/// cable.
pub fn is_param_edge(edge: &Edge) -> bool {
    param_name(&edge.to.port).is_some()
}

/// `<port>.<var>`, the `from.port` of a binding.
pub fn var_port(port: &str, var: &str) -> String {
    format!("{port}.{var}")
}

/// `control.chroma C#` → `("control", "chroma C#")`: split at the first `.`,
/// both halves non-empty.
pub fn split_var_port(from_port: &str) -> Option<(&str, &str)> {
    let (port, var) = from_port.split_once('.')?;
    (!port.is_empty() && !var.is_empty()).then_some((port, var))
}

// ---------------------------------------------------------------------------
// the two types
// ---------------------------------------------------------------------------

/// What a parameter carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    Float,
    /// A generator's colour slot (four RGB stops). Listed so a panel can show
    /// the row; binding and holding one is the colour extension point, not
    /// wired yet.
    Colour,
}

/// One input a node exposes — see the module doc.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// The value the node holds in its config, if it holds one. `None` for a
    /// generator float the digest drives, a script float at its default, a
    /// text effect at its default, and on a catalog entry (which describes a
    /// kind, not a node).
    #[serde(default)]
    pub value: Option<f32>,
    pub kind: ParamKind,
    /// Where it sits: a generator float's position in the row (a colour
    /// slot's first float), a script float's declaration position. `None`
    /// for an effect's `amount`, which is not a slot of its own node.
    #[serde(default)]
    pub index: Option<usize>,
}

/// Equality is reflexive here: every number in a `Param` is finite by
/// construction (manifest floats are JSON numbers, config reads drop anything
/// non-finite), which is what lets [`crate::NodeKind`] — which carries its
/// kind's params — keep deriving `Eq`.
impl Eq for Param {}

impl Param {
    fn float(name: &str, min: f32, max: f32, default: f32, index: Option<usize>) -> Param {
        Param { name: name.to_string(), min, max, default, value: None, kind: ParamKind::Float, index }
    }
}

/// What a variable carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarKind {
    Float,
    /// The picture on an image port — "the render" arriving downstream. Never
    /// the `from` of a binding; listed so a panel can say it is received.
    Image,
    /// Extension point: a colour/palette variable. Nothing produces one yet.
    Colour,
}

/// One value a node offers downstream on one of its output ports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Var {
    pub name: String,
    pub kind: VarKind,
    /// The output port it leaves on (`control`, `row`, `image`).
    pub port: String,
    /// Its position in that port's row (a digest column, a generator float's
    /// row slot, a script output), `None` for an image.
    pub index: Option<usize>,
    pub min: f32,
    pub max: f32,
}

impl Var {
    /// The `from.port` a cable leaving this variable carries: `<port>.<name>`
    /// for a float (what a binding's `from.port` is), the bare port for an
    /// image (what an image cable's `from.port` already is).
    pub fn port_ref(&self) -> String {
        match self.kind {
            VarKind::Image => self.port.clone(),
            VarKind::Float | VarKind::Colour => var_port(&self.port, &self.name),
        }
    }

    fn float(name: &str, port: &str, index: usize, min: f32, max: f32) -> Var {
        Var { name: name.to_string(), kind: VarKind::Float, port: port.to_string(), index: Some(index), min, max }
    }

    /// A colour variable (K): `index` is its slot in the port's row of colours,
    /// `0..1` is the only range a colour carries. `port_ref` is the same
    /// `<port>.<name>` a float's is, so a binding reads the same way.
    fn colour(name: &str, port: &str, index: usize) -> Var {
        Var { name: name.to_string(), kind: VarKind::Colour, port: port.to_string(), index: Some(index), min: 0.0, max: 1.0 }
    }
}

// ---------------------------------------------------------------------------
// what a kind exposes (stamped on the catalog)
// ---------------------------------------------------------------------------

/// The parameters `kind` exposes on its own, before any node config: a
/// generator's floats and colour slots (from the manifest), an effect's
/// `amount`. Empty for every other kind — scripts declare theirs in config.
///
/// **The extension point.** One arm per kind prefix: a later `color`,
/// `palette`, `fx.speed` or `fx.move` kind adds its own arm here, and
/// [`crate::NodeKind::catalog`] stamps it with no other edit.
pub fn kind_params(kind: &str, manifest: &metal_schema::Manifest) -> Vec<Param> {
    // Parity P-A: `gen.ramp` is a source, not a model — its floats are its
    // own, not the manifest's (and `ctl.math` declares its floats in config,
    // like every script preset).
    if kind == crate::RAMP_KIND {
        return parity_params(kind);
    }
    // M1 (plan "Value sources"): a `source.*` node's floats are its own, held
    // under their own config keys like `gen.ramp`'s — not a model's.
    if crate::source::is_source_kind(kind) {
        return source_params(kind);
    }
    // Fixtures F1: the per-fixture node's own config (`venue`/`fixture` and
    // the colour source plus the bus placement). Its numeric knobs are params;
    // the two identifiers and the three mode selectors are strings the panel
    // and the lint read straight off the config.
    if kind == crate::VENUE_FIXTURE_KIND {
        return venue_fixture_params();
    }
    if let Some(model) = kind.strip_prefix("gen.") {
        return manifest.models.iter().find(|m| m.id == model).map(model_params).unwrap_or_default();
    }
    if kind.starts_with("fx.") {
        return effect_params(kind);
    }
    // K: a colour's HSB, so a script or the digest can drive it — the picker
    // writes the same three numbers plus `rgb` below.
    if kind == crate::COLOR_KIND {
        return vec![
            Param::float("h", 0.0, 360.0, 0.0, None),
            Param::float("s", 0.0, 1.0, 1.0, None),
            Param::float("b", 0.0, 1.0, 1.0, None),
        ];
    }
    // K: a palette's whole-set rotation. `-180..180` because a hue is an angle
    // and half a turn either way is every distinct rotation there is; the sign
    // picks the direction the wheel turns.
    if kind == crate::PALETTE_KIND {
        return vec![Param::float("shift", -180.0, 180.0, 0.0, None)];
    }
    // 2026-10-01: `reel.batch`'s stepping, appended in this order and never
    // reordered (rule 2). `unit` and `order` are stepped floats: `unit` 0
    // seconds / 1 beats; `order` 0 sequential / 0.5 loop / 1 random. The
    // defaults are what a batch with no held value does — a step every eight
    // seconds, looping the list.
    if kind == crate::REEL_BATCH_KIND {
        return vec![
            Param::float("interval", 0.05, 600.0, 8.0, None),
            Param::float("unit", 0.0, 1.0, 0.0, None),
            Param::float("order", 0.0, 1.0, 0.5, None),
            Param::float("seed", 0.0, 1000.0, 0.0, None),
        ];
    }
    // Extension point: `color`, `palette`, … — one arm each, above this line.
    Vec::new()
}

/// The `venue.fixture` node's config keys with the value a new node starts
/// with (F1, decision 1): the node is a **pointer** at a venue row (`venue`
/// and `fixture`), how it colours its head (`colour_from`, `spread`, `pick`,
/// `offset`, `hue_lock`) and how it writes the bus (`priority`, `mode`, read
/// by `metal_perform::dmx_bus::bus_source`). One list so the catalog, the
/// panel and the tests cannot drift (rule 7); it never copies the row's
/// address, universe, profile or pose.
pub fn venue_fixture_default_config() -> Value {
    json!({
        "venue": "",
        "fixture": "",
        "colour_from": "frame",
        "drive": "colour",
        "spread": "slots",
        "pick": 0,
        "offset": 0,
        "hue_lock": "free",
        "priority": 100,
        "mode": "htp",
    })
}

/// Every config key a `venue.fixture` node reads, in the order
/// [`venue_fixture_default_config`] lists them.
pub const VENUE_FIXTURE_CONFIG_KEYS: &[&str] = &[
    "venue", "fixture", "colour_from", "drive", "spread", "pick", "offset", "hue_lock", "priority", "mode",
];

/// The colour sources a `venue.fixture` node may name (decision 3).
pub const VENUE_FIXTURE_COLOUR_FROM: &[&str] = &["frame", "model", "signal"];
/// Which channels a patch-driven node packs: colour only, or the whole signal.
pub const VENUE_FIXTURE_DRIVE: &[&str] = &["colour", "patch"];
/// How a rig splits a model's four colour slots (decision 5).
pub const VENUE_FIXTURE_SPREAD: &[&str] = &["slots", "same"];
/// Whether a fixture keeps its own base hue (decision 6).
pub const VENUE_FIXTURE_HUE_LOCK: &[&str] = &["free", "hold"];

/// The drivable, numeric half of [`venue_fixture_default_config`]: `pick`,
/// `offset` and the bus `priority` as catalog params, each with its declared
/// range — the rows a binding may drive. The two identifiers and the three
/// mode selectors are strings the panel and the lint read straight off the
/// config.
pub fn venue_fixture_params() -> Vec<Param> {
    let d = venue_fixture_default_config();
    let numeric = |name: &str, min: f32, max: f32| {
        Param::float(name, min, max, d[name].as_f64().unwrap_or(0.0) as f32, None)
    };
    vec![
        numeric("pick", 0.0, 3.0),
        numeric("offset", 0.0, 3.0),
        numeric("priority", 0.0, 255.0),
    ]
}

/// `fx.forge`'s pinnable ingest columns, in digest order (the first four of
/// `metal_render::models::forge`'s uniform `floats2`): a config key with one of
/// these names replaces that column of what the shader is handed.
pub const FORGE_FX_PARAMS: [&str; 4] = ["rms", "bandwidth", "onset", "centroid"];

/// An `fx.*` kind's parameters, which need no manifest:
///
/// - a common effect (`metal_schema::EFFECT_NAMES`): one `amount` with the
///   effect's own `EFFECTS_FLOATS` range — the value `FxConfig::fixed` holds;
/// - `fx.estela` (and the retired `fx.quimica`): one `amount` in `0..1` — a
///   chain step's mix, which `post.wgsl` clamps to `0..1`;
/// - `fx.text.*`: one `amount` in `0..1`, default `1` — the knob
///   `metal_perform`'s text pass reads off `config.amount`;
/// - `fx.forge`: no amount — its runtime shader reads the digest itself; the
///   four [`FORGE_FX_PARAMS`] pin ingest columns when held (loops T11);
/// - an `fx.*` kind this list does not know (a later `fx.speed`, `fx.move`):
///   none, until it gets its own arm (extension point).
pub fn effect_params(kind: &str) -> Vec<Param> {
    let Some(name) = kind.strip_prefix("fx.") else { return Vec::new() };
    if name.starts_with("text.") {
        return vec![Param::float("amount", 0.0, 1.0, 1.0, None)];
    }
    // T11 (loops): `fx.forge`'s runtime shader reads the ingest itself; these
    // four are the ingest columns a loop (or an operator) may pin, each under
    // its own config key. Absent, the shader gets the digest as it arrives.
    if kind == crate::FORGE_EFFECT_KIND {
        return FORGE_FX_PARAMS.iter().map(|n| Param::float(n, 0.0, 1.0, 0.0, None)).collect();
    }
    // K: `fx.speed` scales its generator's clock. `0..8`, forward only, `1`
    // the identity — reverse is not something a source can play back.
    if kind == format!("fx.{}", crate::SPEED_EFFECT_ID) {
        return vec![Param::float("speed", 0.0, 8.0, 1.0, None)];
    }
    // K: `fx.move` — offset, drift per beat, and wrap. Five numbers, so no
    // folder of its own (the generic parameters table shows them).
    if kind == format!("fx.{}", crate::MOVE_EFFECT_ID) {
        return vec![
            Param::float("x", -1.0, 1.0, 0.0, None),
            Param::float("y", -1.0, 1.0, 0.0, None),
            Param::float("vx", -1.0, 1.0, 0.0, None),
            Param::float("vy", -1.0, 1.0, 0.0, None),
            Param::float("wrap", 0.0, 1.0, 0.0, None),
        ];
    }
    // B: `fx.particles` — stateless dust. Nine floats plus the `dust` colour
    // slot, each held under its own config key like `fx.move`'s five.
    if kind == format!("fx.{}", crate::PARTICLES_EFFECT_ID) {
        return vec![
            Param::float("count", 0.0, 1.0, 0.3, None),
            Param::float("size", 0.0, 1.0, 0.5, None),
            Param::float("drift", 0.0, 1.0, 0.1, None),
            Param::float("direction", 0.0, 360.0, 0.0, None),
            Param::float("jitter", 0.0, 1.0, 0.2, None),
            Param::float("twinkle", 0.0, 1.0, 0.5, None),
            Param::float("glow", 0.0, 1.0, 0.3, None),
            Param::float("depth", 0.0, 1.0, 0.5, None),
            Param::float("seed", 0.0, 1000.0, 0.0, None),
            Param {
                name: "dust".to_string(),
                min: 0.0,
                max: 1.0,
                default: 1.0,
                value: None,
                kind: ParamKind::Colour,
                index: None,
            },
        ];
    }
    // B: `fx.image-pop` — reel media. Twelve floats, each under its own key;
    // the reel itself is the node's `config.reel`, not a float.
    if kind == format!("fx.{}", crate::IMAGE_POP_EFFECT_ID) {
        return vec![
            Param::float("rate", 0.0, 20.0, 2.0, None),
            Param::float("concurrent", 1.0, 32.0, 8.0, None),
            Param::float("life", 0.1, 20.0, 3.0, None),
            Param::float("fade", 0.0, 0.5, 0.2, None),
            Param::float("scale", 0.0, 2.0, 0.5, None),
            Param::float("scale_jitter", 0.0, 1.0, 0.2, None),
            Param::float("spread", 0.0, 1.0, 0.5, None),
            Param::float("position_x", 0.0, 1.0, 0.5, None),
            Param::float("position_y", 0.0, 1.0, 0.5, None),
            Param::float("seed", 0.0, 1000.0, 0.0, None),
            Param::float("blend", 0.0, 1.0, 0.0, None),
            Param::float("crop_zoom", 0.0, 1.0, 0.0, None),
        ];
    }
    // Loops T12: `fx.warp`'s mesh generators, every one `0..1`.
    if kind == crate::WARP_EFFECT_KIND {
        return metal_schema::warp::PARAMS
            .iter()
            .map(|p| Param::float(p.name, p.min, p.max, p.default, None))
            .collect();
    }
    // Parity P-A: the five image effects, each float under its own key.
    if crate::parity::is_parity_effect(kind) {
        return parity_params(kind);
    }
    // Loops T13: `fx.flash` — how far toward white, and how diffuse the
    // blend (0 screen, 1 soft light).
    if kind == crate::FLASH_EFFECT_KIND {
        return vec![Param::float("opacity", 0.0, 1.0, 0.0, None), Param::float("diffuse", 0.0, 1.0, 0.5, None)];
    }
    // The advanced effects (grade, texture, clarity, dehaze, denoise, optics,
    // geometry): one param per float of the group, in declared order, held
    // each under its own config key (like `fx.move`). The FloatSpec carries
    // the name/range/default; `index` stays `None` because none is a single
    // `amount` slot.
    if let Some(floats) = metal_schema::advanced_effect_floats(name) {
        return floats
            .iter()
            .map(|s| Param::float(s.name, s.min, s.max, s.default, None))
            .collect();
    }
    if let Some(i) = metal_schema::effect_index(name) {
        let s = &metal_schema::EFFECTS_FLOATS[i];
        return vec![Param::float("amount", s.min, s.max, s.default, None)];
    }
    if metal_schema::footage_chain_kind(name).is_some() {
        return vec![Param::float("amount", 0.0, 1.0, 0.0, None)];
    }
    Vec::new()
}

/// Whether `kind` is one of the advanced `fx.*` effects (`fx.grade`,
/// `fx.texture`, …) that hold each parameter under its own config key rather
/// than a single `fixed` amount.
fn is_advanced_effect(kind: &str) -> bool {
    kind.strip_prefix("fx.")
        .is_some_and(|id| metal_schema::ADVANCED_EFFECT_IDS.contains(&id))
}

/// A block of a model's row, reduced to what this walk needs — the shape both
/// representations of a row ([`metal_schema::ManifestBlock`] on disk,
/// [`metal_schema::Block`] at runtime) come down to, so the rule that picks a
/// generator's parameters is written once.
enum RowBlock<'a> {
    Floats(Vec<(&'a str, f32, f32, f32)>),
    Colour(&'a str, f32),
    Other(usize),
}

/// A generator's parameters from its manifest entry: every float of its own
/// `Floats` blocks and every colour slot, each with its position in the row —
/// **without** the shared `sig_*` block (driven by `out.*` nodes) and the
/// shared effects block and its `leak_color` slot (driven by `fx.*` nodes).
pub fn model_params(model: &metal_schema::ManifestModel) -> Vec<Param> {
    let camera = metal_schema::camera([0.0; 3], [0.0; 3], 0.0).width();
    row_params(model.blocks.iter().map(|b| match b {
        metal_schema::ManifestBlock::Floats { fields } => {
            RowBlock::Floats(fields.iter().map(|f| (f.name.as_str(), f.min, f.max, f.default)).collect())
        }
        metal_schema::ManifestBlock::ColorSlot { name, default } => RowBlock::Colour(
            name.as_str(),
            default.as_ref().and_then(|d| d.first().copied()).unwrap_or(metal_schema::WHITE4[0]),
        ),
        metal_schema::ManifestBlock::Camera { .. } => RowBlock::Other(camera),
    }))
}

/// [`model_params`] off a runtime [`metal_schema::Schema`] — what the painter
/// actually lays a row out by. The same rule over the other shape of the same
/// data; a test holds the two equal for every model in the manifest.
pub fn schema_params(schema: &metal_schema::Schema) -> Vec<Param> {
    row_params(schema.blocks.iter().map(|b| match b {
        metal_schema::Block::Floats(specs) => {
            RowBlock::Floats(specs.iter().map(|s| (s.name, s.min, s.max, s.default)).collect())
        }
        metal_schema::Block::ColorSlot(name, start) => RowBlock::Colour(name, start[0]),
        other => RowBlock::Other(other.width()),
    }))
}

fn row_params<'a>(blocks: impl Iterator<Item = RowBlock<'a>>) -> Vec<Param> {
    let colour_width = metal_schema::color("").width();
    let effects_colour = match metal_schema::EFFECTS_COLOR_BLOCK {
        metal_schema::Block::ColorSlot(name, _) => name,
        _ => "",
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    for block in blocks {
        match block {
            RowBlock::Floats(fields) => {
                if !is_shared_block(&fields) {
                    for (k, (name, min, max, default)) in fields.iter().enumerate() {
                        out.push(Param::float(name, *min, *max, *default, Some(at + k)));
                    }
                }
                at += fields.len();
            }
            RowBlock::Colour(name, default) => {
                if name != effects_colour {
                    out.push(Param {
                        name: name.to_string(),
                        min: 0.0,
                        max: 1.0,
                        default,
                        value: None,
                        kind: ParamKind::Colour,
                        index: Some(at),
                    });
                }
                at += colour_width;
            }
            RowBlock::Other(width) => at += width,
        }
    }
    out
}

/// Whether a `Floats` block is one of the two every model carries — the
/// `sig_*` block or the effects block — told apart by its names, which is the
/// only thing both representations agree on (a manifest-built schema's slices
/// are not `metal_schema`'s own statics, so `metal_schema::is_effects_block`'s
/// pointer test cannot see them).
fn is_shared_block(fields: &[(&str, f32, f32, f32)]) -> bool {
    let same = |specs: &[metal_schema::FloatSpec]| {
        specs.len() == fields.len() && specs.iter().zip(fields).all(|(s, f)| s.name == f.0)
    };
    same(metal_schema::SIGNAL_FLOATS) || same(metal_schema::EFFECTS_FLOATS)
}

/// Stamps [`kind_params`] on every catalog entry — called once, at the end of
/// [`crate::NodeKind::catalog`], so a kind pushed anywhere above that line is
/// covered.
pub(crate) fn stamp(catalog: &mut [NodeKind], manifest: &metal_schema::Manifest) {
    for k in catalog.iter_mut() {
        k.params = kind_params(&k.kind, manifest);
    }
}

// ---------------------------------------------------------------------------
// what one node exposes
// ---------------------------------------------------------------------------

fn config_json(node: &Node) -> Value {
    match &node.config.0 {
        Value::Null => json!({}),
        v => v.clone(),
    }
}

fn number(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32).filter(|f| f.is_finite())
}

/// A palette node's resolved colours, `config.colors` — the list the canvas
/// paints and the slots the node offers. A malformed entry is skipped rather
/// than guessed at, the same lenient reading [`script_outputs`] gives.
pub fn palette_colors(node: &Node) -> Vec<[f32; 3]> {
    let config = config_json(node);
    config
        .get("colors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let arr = c.as_array()?;
                    Some([
                        number(arr.first()?)?,
                        number(arr.get(1)?)?,
                        number(arr.get(2)?)?,
                    ])
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A script's declared floats, leniently: an entry without a `name` is
/// skipped, a missing range reads `0..1` (the same fallback
/// `script_floats::apply_floats_patch` uses), a missing `default` reads `min`.
fn script_params(config: &Value) -> Vec<Param> {
    let Some(floats) = config.get("floats").and_then(Value::as_array) else { return Vec::new() };
    floats
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let name = f.get("name")?.as_str()?;
            let min = f.get("min").and_then(number).unwrap_or(0.0);
            let max = f.get("max").and_then(number).unwrap_or(1.0);
            let default = f.get("default").and_then(number).unwrap_or(min);
            let mut p = Param::float(name, min, max, default, Some(i));
            p.value = f.get("value").and_then(number);
            Some(p)
        })
        .collect()
}

/// The parameters one node exposes: its catalog entry's (`kind`), plus what
/// its config declares (a script's floats), with every held value read off
/// the config — see the module doc for where each kind holds it.
pub fn node_params(node: &Node, kind: Option<&NodeKind>) -> Vec<Param> {
    let config = config_json(node);
    let mut out: Vec<Param> = kind.map(|k| k.params.clone()).unwrap_or_default();
    if node.kind == crate::RAMP_KIND || crate::parity::is_parity_effect(&node.kind) {
        // Parity P-A: every float under its own config key, like `fx.move`.
        for p in out.iter_mut().filter(|p| p.kind == ParamKind::Float) {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    } else if crate::source::is_source_kind(&node.kind) {
        // M1: a `source.*` float under its own config key — the same held
        // reading as `gen.ramp`'s.
        for p in out.iter_mut().filter(|p| p.kind == ParamKind::Float) {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    } else if node.kind.starts_with("gen.") {
        let held = config.get(GEN_HELD_KEY).and_then(Value::as_object);
        for p in out.iter_mut().filter(|p| p.kind == ParamKind::Float) {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| held.and_then(|h| h.get(k))).and_then(number);
        }
    } else if node.kind.starts_with("fx.text.") {
        for p in out.iter_mut() {
            p.value = config.get("amount").and_then(number);
        }
    } else if node.kind == format!("fx.{}", crate::MOVE_EFFECT_ID)
        || node.kind == format!("fx.{}", crate::PARTICLES_EFFECT_ID)
        || node.kind == format!("fx.{}", crate::IMAGE_POP_EFFECT_ID)
        || is_advanced_effect(&node.kind)
    {
        // K/B/advanced: `fx.move`, `fx.particles` and `fx.image-pop`, and each
        // advanced effect, hold every parameter under its own config key (the
        // shader/compositor reads them off the node), so a missing key is an
        // unheld parameter shown at its declared default. A colour slot has no
        // scalar to read here.
        for p in out.iter_mut().filter(|p| p.kind == ParamKind::Float) {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    } else if node.kind.starts_with("fx.") {
        // `FxConfig::fixed` always has a value (0 when never written): an
        // effect with nothing driving it shows exactly that. `fx.speed` is one
        // `fixed` too — its `speed` is the amount `crate::bind` writes.
        let fixed = config.get("fixed").and_then(number).unwrap_or(0.0);
        for p in out.iter_mut() {
            p.value = Some(fixed);
        }
    } else if crate::is_script_kind(&node.kind) {
        out.extend(script_params(&config));
    } else if node.kind == crate::COLOR_KIND {
        // K: `h`/`s`/`b`/`shift` are held plainly under their own config keys
        // (the picker writes them beside `rgb`/`colors`). A missing key is an
        // unheld parameter, shown at its declared default.
        for p in out.iter_mut() {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    } else if node.kind == crate::PALETTE_KIND || node.kind == crate::REEL_BATCH_KIND {
        for p in out.iter_mut() {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    } else if node.kind == crate::VENUE_FIXTURE_KIND {
        // F1: `pick`/`offset`/`priority` are held under their own config keys
        // (the same keys the runtime reads), so a missing key is an unheld
        // parameter at its declared default.
        for p in out.iter_mut().filter(|p| p.kind == ParamKind::Float) {
            p.value = metal_schema::param_aliases::alias_keys(&p.name).find_map(|k| config.get(k)).and_then(number);
        }
    }
    out
}

/// One parameter of one node, by name — the float first when a colour slot
/// shares its name (see the module doc).
pub fn find_param(node: &Node, kind: Option<&NodeKind>, name: &str) -> Option<Param> {
    let name = metal_schema::param_aliases::canonical_param_name(name);
    let params = node_params(node, kind);
    let float = params.iter().position(|p| p.name == name && p.kind == ParamKind::Float);
    let any = params.iter().position(|p| p.name == name);
    float.or(any).map(|i| params[i].clone())
}

/// A script's declared outputs, `config.outputs`: `["x", "y"]` or
/// `[{"name": "x", "min": 0, "max": 2}]` (either form, mixed; a missing range
/// reads `0..1`, the range of every control value). Read-compatible: a script
/// that declares none has none, and its outputs are `out0..outN-1`.
fn script_outputs(config: &Value) -> Vec<(String, f32, f32)> {
    let Some(outputs) = config.get("outputs").and_then(Value::as_array) else { return Vec::new() };
    outputs
        .iter()
        .filter_map(|o| match o {
            Value::String(name) if !name.is_empty() => Some((name.clone(), 0.0, 1.0)),
            Value::Object(m) => {
                let name = m.get("name")?.as_str().filter(|n| !n.is_empty())?.to_string();
                let min = m.get("min").and_then(number).unwrap_or(0.0);
                let max = m.get("max").and_then(number).unwrap_or(1.0);
                Some((name, min, max))
            }
            _ => None,
        })
        .collect()
}

/// `outN` → `N`, only in its canonical spelling (`out01` is not `out1`).
fn dynamic_index(name: &str) -> Option<usize> {
    let n: usize = name.strip_prefix("out")?.parse().ok()?;
    (format!("out{n}") == name).then_some(n)
}

/// Whether `kind`'s `control` output is sized live — a script's (beyond what
/// it declares) and a sensor's: any canonical `outN` is a valid variable.
fn live_sized(kind: &str) -> bool {
    crate::is_script_kind(kind) || kind.starts_with("sensor.") || kind == crate::ARTNET_IN_KIND
}

/// How many digest columns an `audio.*` `control` carries — the count of
/// [`metal_schema::DIGEST_COLUMNS`], spelled once so the two Looper variables
/// below are indexed past it ([`REPEAT_VARS`]).
pub const DIGEST_COLUMNS_LEN: usize = metal_schema::DIGEST_COLUMNS.len();

/// The two `audio.*` `control` variables that are not digest columns: what
/// the Looper is doing this frame, appended after the sixteen columns, `0..1`.
/// Their values come from `metal_perform`'s Looper at binding time, not from the
/// ingest row — see `metal_perform::bind::RepeatVars`.
pub const REPEAT_VARS: [&str; 2] = ["repeat.locked", "repeat.phase"];

/// The variables one node offers, port by port:
///
/// - every image output port: its picture ([`VarKind::Image`]);
/// - `audio.*` `control`: the sixteen digest columns
///   ([`metal_schema::DIGEST_COLUMNS`]), `0..1`, then the two Looper variables
///   `repeat.locked`/`repeat.phase` ([`REPEAT_VARS`]), also `0..1` — appended
///   past the columns so every digest index stays what it was;
/// - `gen.*` `row`: the generator's float parameters, same names, ranges and
///   row slots;
/// - `script.*` `control`: its declared outputs ([`script_outputs`]), then
///   `out<k>` up to `live_width` — the length of the row it answered last
///   frame, when the caller knows it;
/// - `sensor.*` `control`: `out0..` up to `live_width` (extension point for
///   colour/palette variables).
pub fn node_vars(node: &Node, kind: Option<&NodeKind>, live_width: Option<usize>) -> Vec<Var> {
    let mut out = Vec::new();
    if let Some(k) = kind {
        for p in k.outputs.iter().filter(|p| p.ty == PortType::Image) {
            out.push(Var { name: p.name.clone(), kind: VarKind::Image, port: p.name.clone(), index: None, min: 0.0, max: 1.0 });
        }
    }
    if node.kind.starts_with("audio.") {
        for (i, name) in metal_schema::DIGEST_COLUMNS.iter().enumerate() {
            out.push(Var::float(name, "control", i, 0.0, 1.0));
        }
        // The Looper's two floats, appended (never in the middle): a digest
        // index is the column's row slot, so inserting would reindex every
        // column a binding addresses by name. `metal_perform`'s `bind` supplies
        // their values off the Looper — they are not digest columns.
        for (i, name) in REPEAT_VARS.iter().enumerate() {
            out.push(Var::float(name, "control", DIGEST_COLUMNS_LEN + i, 0.0, 1.0));
        }
    } else if node.kind == crate::COLOR_KIND {
        // K: the colour itself, then its three HSB floats (a script or the
        // digest can drive those). The colour first, so a panel lists it where
        // the operator looks for it.
        out.push(Var::colour("rgb", "control", 0));
        out.push(Var::float("h", "control", 1, 0.0, 360.0));
        out.push(Var::float("s", "control", 2, 0.0, 1.0));
        out.push(Var::float("b", "control", 3, 0.0, 1.0));
    } else if node.kind == crate::PALETTE_KIND {
        // K: one Colour variable per slot the node holds — the stored four
        // (`base, acento, fondo, destello`) plus however many `extra` colours
        // its config carries, in the same order `colors` does.
        let colors = palette_colors(node);
        for i in 0..colors.len().max(crate::PALETTE_SLOTS.len()) {
            out.push(Var::colour(&crate::palette_slot_var(i), "control", i));
        }
    } else if node.kind.starts_with("gen.") {
        for p in kind.map(|k| k.params.as_slice()).unwrap_or_default() {
            if p.kind == ParamKind::Float {
                out.push(Var::float(&p.name, "row", p.index.unwrap_or(0), p.min, p.max));
            }
        }
        // Fixtures F4: a model's driven colour slot is exposed to the graph as
        // four `control` colours (`colour0..colour3`), so a `venue.fixture`
        // node can read what the model decided (decision 4) instead of
        // re-analysing audio. `index` is the slot's row offset, where
        // `drive::row_with` filled `Block::ColorSlot`.
        if let Some(p) = kind
            .and_then(|k| k.params.iter().find(|p| p.kind == ParamKind::Colour))
        {
            let start = p.index.unwrap_or(0);
            for i in 0..4 {
                out.push(Var::colour(&format!("colour{i}"), "control", start + i * 3));
            }
        }
    } else if crate::source::is_source_kind(&node.kind) {
        // M1: a `source.*` offers `out0..out{per-1}` on `control`, each with
        // the range its own config implies (see `crate::source`). The count is
        // known from the config, not from a live frame.
        let (min, max) = crate::source::output_range(node);
        for i in 0..crate::source::output_count(node) {
            out.push(Var::float(&format!("out{i}"), "control", i, min, max));
        }
    } else if live_sized(&node.kind) {
        let declared = if crate::is_script_kind(&node.kind) { script_outputs(&config_json(node)) } else { Vec::new() };
        for (i, (name, min, max)) in declared.iter().enumerate() {
            out.push(Var::float(name, "control", i, *min, *max));
        }
        for i in declared.len()..live_width.unwrap_or(0) {
            out.push(Var::float(&format!("out{i}"), "control", i, 0.0, 1.0));
        }
    }
    out
}

/// The variable a binding's `from.port` names on `node` — the check lint and
/// [`crate::bind::bind`] share. A live-sized port (a script's or a sensor's
/// `control`) accepts any canonical `out<k>` past what the script declares,
/// since how many values it answers is only known while it runs.
pub fn resolve_var(node: &Node, kind: Option<&NodeKind>, from_port: &str) -> Result<Var, String> {
    let Some((port, name)) = split_var_port(from_port) else {
        return Err(format!(
            "{from_port:?} is not a variable — a binding leaves `<port>.<variable>` (e.g. `control.rms`)"
        ));
    };
    let Some(k) = kind else {
        return Err(format!("{} is a {}, which the catalog does not know", node.id, node.kind));
    };
    match k.outputs.iter().find(|p| p.name == port) {
        Some(p) if p.ty == PortType::Control => {}
        Some(_) => {
            return Err(format!(
                "{}.{port} carries a picture, not numbers — only a control port's variables can drive a parameter",
                node.id
            ))
        }
        None => return Err(format!("{} ({}) has no output port {port:?}", node.id, node.kind)),
    }
    if let Some(v) = node_vars(node, kind, None).into_iter().find(|v| v.port == port && v.name == name) {
        return Ok(v);
    }
    if live_sized(&node.kind) {
        if let Some(i) = dynamic_index(name) {
            let declared = if crate::is_script_kind(&node.kind) { script_outputs(&config_json(node)) } else { Vec::new() };
            if let Some((named, _, _)) = declared.get(i) {
                return Err(format!(
                    "{} names its output {i} `{named}` — bind `{port}.{named}`",
                    node.id
                ));
            }
            return Ok(Var::float(name, port, i, 0.0, 1.0));
        }
    }
    Err(format!("{} has no variable {name:?} on its {port} port", node.id))
}

// ---------------------------------------------------------------------------
// what is upstream and downstream
// ---------------------------------------------------------------------------

/// Every node upstream of `node` — anything with a path of cables into it,
/// bindings included — with its distance in hops, nearest first (ties by
/// id). Cycle-safe: each node is reached once, at its shortest distance, and
/// `node` itself is never listed.
pub fn upstream(graph: &Graph, node: &str) -> Vec<(NodeId, usize)> {
    walk(graph, node, true)
}

/// The mirror of [`upstream`]: every node `node` reaches.
pub fn downstream(graph: &Graph, node: &str) -> Vec<(NodeId, usize)> {
    walk(graph, node, false)
}

fn walk(graph: &Graph, start: &str, up: bool) -> Vec<(NodeId, usize)> {
    let mut seen: HashSet<&str> = HashSet::from([start]);
    let mut frontier: Vec<&str> = vec![start];
    let mut out = Vec::new();
    let mut hops = 0;
    while !frontier.is_empty() {
        hops += 1;
        let mut next: BTreeSet<&str> = BTreeSet::new();
        for at in &frontier {
            for e in &graph.edges {
                let (here, there) = if up { (&e.to.node, &e.from.node) } else { (&e.from.node, &e.to.node) };
                if here == at && graph.node(there).is_some() && !seen.contains(there.as_str()) {
                    next.insert(there.as_str());
                }
            }
        }
        for id in &next {
            seen.insert(id);
            out.push((id.to_string(), hops));
        }
        frontier = next.into_iter().collect();
    }
    out
}

// ---------------------------------------------------------------------------
// the one writer of a held value
// ---------------------------------------------------------------------------

/// Sets (or, with `None`, releases) the value `node` holds for parameter
/// `name` — the one writer behind `POST /graph/node {op: "param"}`, the AI's
/// `set_param` and a single float through `set_params`. Refuses an unknown
/// parameter, a colour one (extension point) and an out-of-range value by
/// name, and writes nothing then; it never clamps on write (the same rule as
/// [`crate::script_floats::apply_floats_patch`], which is what it calls for a
/// script).
///
/// Releasing: a generator float goes back to the digest drive, a script float
/// and a text effect to their declared default, an effect's fixed amount to
/// its declared default.
pub fn set_held(
    graph: &mut Graph,
    node: &str,
    name: &str,
    value: Option<f32>,
    catalog: &[NodeKind],
) -> Result<(), String> {
    let n = graph.node(node).ok_or_else(|| format!("no such node: {node}"))?;
    let kind = catalog.iter().find(|k| k.kind == n.kind);
    let Some(param) = find_param(n, kind, name) else {
        return Err(format!("{node} ({}) has no parameter {name:?}", n.kind));
    };
    if param.kind == ParamKind::Colour {
        return Err(format!(
            "{node}.{name} is a colour slot — holding a colour per node is not wired yet"
        ));
    }
    if let Some(v) = value {
        if !v.is_finite() || v < param.min || v > param.max {
            return Err(format!(
                "{node}.{name}: {v} is outside its declared [{}, {}]",
                param.min, param.max
            ));
        }
    }
    let node_kind = n.kind.clone();
    let mut config = config_json(n);
    // K: a colour's `h`/`s`/`b` and a palette's `shift` are held under their
    // own config keys — the picker and the digest both write there; releasing
    // removes the key so the parameter falls back to its default.
    if node_kind == crate::COLOR_KIND || node_kind == crate::PALETTE_KIND || node_kind == crate::REEL_BATCH_KIND {
        match value {
            Some(v) => config[name] = json!(v),
            None => {
                if let Some(o) = config.as_object_mut() {
                    o.remove(name);
                }
            }
        }
        graph.node_mut(node).expect("found above").config = Config::from_value(config);
        return Ok(());
    }
    // F1: `venue.fixture`'s `pick`/`offset`/`priority` are held under their own
    // config keys, beside the pointer and the mode selectors it also carries.
    if node_kind == crate::VENUE_FIXTURE_KIND {
        match value {
            Some(v) => config[name] = json!(v),
            None => {
                if let Some(o) = config.as_object_mut() {
                    o.remove(name);
                }
            }
        }
        graph.node_mut(node).expect("found above").config = Config::from_value(config);
        return Ok(());
    }
    if node_kind != crate::RAMP_KIND && node_kind.starts_with("gen.") {
        let mut held: Map<String, Value> =
            config.get(GEN_HELD_KEY).and_then(Value::as_object).cloned().unwrap_or_default();
        // One key per parameter: the canonical name. A value written under an
        // alias (`tam`, `tamano` for `size`) while the canonical key still holds
        // an old one would be read back stale, since the reader tries the
        // canonical name first.
        let canonical = metal_schema::param_aliases::canonical_param_name(name).to_string();
        for k in metal_schema::param_aliases::alias_keys(name).map(str::to_string).collect::<Vec<_>>() {
            held.remove(&k);
        }
        match value {
            Some(v) => held.insert(canonical, json!(v)),
            None => None,
        };
        config[GEN_HELD_KEY] = Value::Object(held);
    } else if node_kind.starts_with("fx.text.") {
        match value {
            Some(v) => config["amount"] = json!(v),
            None => {
                if let Some(o) = config.as_object_mut() {
                    o.remove("amount");
                }
            }
        }
    } else if node_kind == format!("fx.{}", crate::MOVE_EFFECT_ID)
        || node_kind == format!("fx.{}", crate::PARTICLES_EFFECT_ID)
        || node_kind == format!("fx.{}", crate::IMAGE_POP_EFFECT_ID)
        || is_advanced_effect(&node_kind)
        || node_kind == crate::RAMP_KIND
        || crate::parity::is_parity_effect(&node_kind)
        || crate::source::is_source_kind(&node_kind)
    {
        // K/B/advanced: `fx.move`, `fx.particles` and `fx.image-pop`, and each
        // advanced effect, hold every parameter under its own key; releasing
        // one removes it so it falls back to its declared default.
        match value {
            Some(v) => config[name] = json!(v),
            None => {
                if let Some(o) = config.as_object_mut() {
                    o.remove(name);
                }
            }
        }
    } else if node_kind.starts_with("fx.") {
        config["fixed"] = json!(value.unwrap_or(param.default));
    } else if crate::is_script_kind(&node_kind) {
        match value {
            Some(v) => {
                let mut patch = Map::new();
                patch.insert(name.to_string(), json!(v));
                crate::script_floats::apply_floats_patch(&mut config, &patch)?;
            }
            None => {
                if let Some(floats) = config.get_mut("floats").and_then(Value::as_array_mut) {
                    for f in floats.iter_mut() {
                        if f.get("name").and_then(Value::as_str) == Some(name) {
                            if let Some(o) = f.as_object_mut() {
                                o.remove("value");
                            }
                        }
                    }
                }
            }
        }
    } else {
        return Err(format!("{node} ({node_kind}) holds no parameter values"));
    }
    graph.node_mut(node).expect("found above").config = Config::from_value(config);
    Ok(())
}

/// The held generator floats of `node` — `config.floats` as an object — by
/// name. What the runtime pins over the driven row.
pub fn gen_held(node: &Node) -> HashMap<String, f32> {
    let config = config_json(node);
    config
        .get(GEN_HELD_KEY)
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| number(v).map(|v| (k.clone(), v))).collect())
        .unwrap_or_default()
}

/// A script node's declared output names, in order — what its `control`
/// variables are called before `out<k>`. Empty when it declares none.
pub fn script_output_names(node: &Node) -> Vec<String> {
    script_outputs(&config_json(node)).into_iter().map(|(n, _, _)| n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Domain, Endpoint};

    fn manifest() -> metal_schema::Manifest {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        metal_schema::Manifest::parse(&metal_vfs::read_to_string(path).expect("schema.json")).expect("parses")
    }

    fn catalog() -> Vec<NodeKind> {
        NodeKind::catalog(&manifest())
    }

    fn kind<'a>(catalog: &'a [NodeKind], kind: &str) -> &'a NodeKind {
        catalog.iter().find(|k| k.kind == kind).unwrap_or_else(|| panic!("{kind} is cataloged"))
    }

    fn with_config(g: &mut Graph, id: &str, config: Value) {
        g.node_mut(id).unwrap().config = Config::from_value(config);
    }

    #[test]
    fn the_port_syntax_splits_at_the_first_dot_and_names_the_param() {
        assert_eq!(param_port("amount"), "param:amount");
        assert_eq!(param_name("param:amount"), Some("amount"));
        assert_eq!(param_name("control"), None);
        assert_eq!(param_name("param:"), None, "an empty name is no parameter");
        assert_eq!(split_var_port("control.chroma C#"), Some(("control", "chroma C#")));
        assert_eq!(split_var_port("row.a.b"), Some(("row", "a.b")), "only the first dot splits");
        assert_eq!(split_var_port("control"), None);
        assert_eq!(split_var_port(".rms"), None);
        assert_eq!(var_port("control", "rms"), "control.rms");
    }

    /// A generator's parameters are its own floats and colour slots, in row
    /// order with their row slots — never the shared `sig_*`/effects blocks.
    #[test]
    fn a_generators_params_are_its_own_floats_and_colours_without_the_shared_blocks() {
        let m = manifest();
        let catalog = NodeKind::catalog(&m);
        for model in m.models.iter().filter(|m| !m.footage) {
            let params = &kind(&catalog, &format!("gen.{}", model.id)).params;
            let own_floats: usize = model
                .blocks
                .iter()
                .filter_map(|b| match b {
                    // A whole-block match: Corals has an own float named
                    // `shake`, so one shared name must not disown its block.
                    metal_schema::ManifestBlock::Floats { fields }
                        if !is_shared_block(
                            &fields
                                .iter()
                                .map(|f| (f.name.as_str(), f.min, f.max, f.default))
                                .collect::<Vec<_>>(),
                        ) =>
                    {
                        Some(fields.len())
                    }
                    _ => None,
                })
                .sum();
            let floats = params.iter().filter(|p| p.kind == ParamKind::Float).count();
            assert_eq!(floats, own_floats, "gen.{}: one float param per own float", model.id);
            // The shared blocks trail every row (`metal_schema::read_signal`/
            // `read_post` locate them from the end), so no own slot may sit
            // there. Names alone cannot say it: Fractals has a float of its
            // own called `zoom`.
            let width: usize = model
                .blocks
                .iter()
                .map(|b| match b {
                    metal_schema::ManifestBlock::Floats { fields } => fields.len(),
                    metal_schema::ManifestBlock::ColorSlot { .. } => metal_schema::color("").width(),
                    metal_schema::ManifestBlock::Camera { .. } => metal_schema::camera([0.0; 3], [0.0; 3], 0.0).width(),
                })
                .sum();
            let shared_from = width - metal_schema::EFFECTS_WIDTH - metal_schema::SIGNAL_WIDTH;
            for p in params {
                let at = p.index.expect("every generator param has a row slot");
                assert!(at < shared_from, "gen.{}: {} sits in the shared sig_*/effects blocks", model.id, p.name);
                assert!(!p.name.starts_with("sig_"), "gen.{}: {} is a signal float", model.id, p.name);
                assert_ne!(p.name, "leak_color", "gen.{}: the effects' colour slot", model.id);
            }
        }
    }

    /// The same rule over the two shapes of one row: the manifest on disk and
    /// the `Schema` a painter lays the row out by. A drift between the two
    /// walks would bind a float to the wrong slot, in range and silently.
    #[test]
    fn manifest_params_and_schema_params_agree_for_every_model() {
        let m = manifest();
        let models = m.models.clone();
        let suite = m.into_suite();
        for (model, entry) in models.iter().zip(suite.iter()) {
            assert_eq!(model_params(model), schema_params(entry.schema), "{}", model.id);
        }
    }

    /// A row slot is where the row really carries that float: walking the
    /// schema's own defaults, the slot of every float param holds its default.
    #[test]
    fn a_float_params_index_is_its_slot_in_the_row() {
        let m = manifest();
        for entry in m.into_suite() {
            let defaults = entry.schema.defaults();
            for p in schema_params(entry.schema).iter().filter(|p| p.kind == ParamKind::Float) {
                assert_eq!(defaults[p.index.unwrap()], p.default, "{}.{}", entry.id, p.name);
            }
        }
    }

    /// K: a colour's HSB is its params; a palette's `shift` is its one knob —
    /// and neither holds anything until its config says so.
    #[test]
    fn a_colour_exposes_hsb_and_a_palette_its_shift() {
        let catalog = catalog();
        let c = &kind(&catalog, crate::COLOR_KIND).params;
        assert_eq!(c.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["h", "s", "b"]);
        assert_eq!((c[0].min, c[0].max), (0.0, 360.0), "hue is degrees");
        assert_eq!((c[1].min, c[1].max), (0.0, 1.0));
        assert!(c.iter().all(|p| p.value.is_none()), "nothing held yet");
        let p = &kind(&catalog, crate::PALETTE_KIND).params;
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].name.as_str(), p[0].min, p[0].max), ("shift", -180.0, 180.0));
    }

    /// K: a colour offers its `rgb` as a colour and its HSB as floats; a
    /// palette offers one colour per slot it holds.
    #[test]
    fn colour_and_palette_variables() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let color = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        let palette = g.add_node("palette", crate::PALETTE_KIND, [0.0, 0.0]);
        with_config(
            &mut g,
            &palette,
            json!({ "palette": null, "colors": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0], [0.5, 0.5, 0.5]] }),
        );
        let vars = |g: &Graph, id: &str| {
            let n = g.node(id).unwrap();
            node_vars(n, catalog.iter().find(|k| k.kind == n.kind), None)
        };
        let c = vars(&g, &color);
        assert_eq!(c[0].kind, VarKind::Colour);
        assert_eq!(c[0].port_ref(), "control.rgb");
        assert_eq!(c[0].index, Some(0));
        assert_eq!(c[1..].iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["h", "s", "b"]);
        assert!(c[1..].iter().all(|v| v.kind == VarKind::Float));

        let p = vars(&g, &palette);
        assert_eq!(p.len(), 5, "four stored slots plus one extra colour");
        assert_eq!(
            p.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["base", "accent", "background", "flash", "extra0"]
        );
        assert!(p.iter().all(|v| v.kind == VarKind::Colour));
        assert_eq!(p[4].port_ref(), "control.extra0");
    }

    /// A palette with no `colors` still offers the four stored slots (all
    /// grey until written) — a node the operator dropped but has not painted
    /// is still a palette.
    #[test]
    fn an_empty_palette_still_offers_the_four_stored_slots() {
        let catalog = catalog();
        let g0 = {
            let mut g = Graph::new("g", "t", Domain::Live);
            g.add_node("palette", crate::PALETTE_KIND, [0.0, 0.0]);
            g
        };
        let n = g0.nodes.first().unwrap();
        let vars = node_vars(n, catalog.iter().find(|k| k.kind == n.kind), None);
        assert_eq!(vars.len(), crate::PALETTE_SLOTS.len());
        assert_eq!(vars.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), crate::PALETTE_SLOTS);
    }

    /// F4: every model offers its driven colour slot on the `control` port as
    /// `colour0..colour3`, computed from the catalog (rule 7) — the variable a
    /// `venue.fixture` node reads instead of re-analysing audio. A model with
    /// no colour slot offers none.
    #[test]
    fn every_model_offers_its_colour_slot_as_four_control_colours() {
        let catalog = catalog();
        let mut with_slot = 0;
        for k in catalog.iter().filter(|k| k.kind.starts_with("gen.")) {
            let mut g = Graph::new("g", "t", Domain::Live);
            g.add_node("gen", &k.kind, [0.0, 0.0]);
            let n = g.nodes.first().unwrap();
            let vars = node_vars(n, Some(k), None);
            let colours: Vec<&str> = vars
                .iter()
                .filter(|v| v.port == "control" && v.name.starts_with("colour"))
                .map(|v| v.name.as_str())
                .collect();
            if k.params.iter().any(|p| p.kind == ParamKind::Colour) {
                assert_eq!(colours, ["colour0", "colour1", "colour2", "colour3"], "{}", k.kind);
                with_slot += 1;
            } else {
                assert!(colours.is_empty(), "{} declares no colour slot", k.kind);
            }
        }
        assert!(with_slot > 0, "the catalog must carry a model with a colour slot");
    }

    /// K: the slot helpers are inverses — the catalog's own vocabulary.
    #[test]
    fn palette_slot_names_round_trip() {
        for (i, name) in crate::PALETTE_SLOTS.iter().enumerate() {
            assert_eq!(crate::palette_slot_index(name), Some(i));
            assert_eq!(crate::palette_slot_var(i), *name);
        }
        assert_eq!(crate::palette_slot_var(4), "extra0");
        assert_eq!(crate::palette_slot_index("extra0"), Some(4));
        assert_eq!(crate::palette_slot_index("extra07"), None, "only the canonical spelling");
        assert_eq!(crate::palette_slot_index("nope"), None);
    }

    /// K: a held `h`/`s`/`b` reads back off the config, and releasing it
    /// removes the key.
    #[test]
    fn a_colours_hsb_is_held_in_its_config() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let c = g.add_node("color", crate::COLOR_KIND, [0.0, 0.0]);
        with_config(&mut g, &c, json!({ "rgb": [1.0, 0.0, 0.0], "h": 0.0, "s": 1.0, "b": 1.0 }));
        let find = |g: &Graph, name: &str| {
            let n = g.node(&c).unwrap();
            find_param(n, catalog.iter().find(|k| k.kind == n.kind), name).unwrap()
        };
        assert_eq!(find(&g, "h").value, Some(0.0));
        assert_eq!(find(&g, "s").value, Some(1.0));
        set_held(&mut g, &c, "b", Some(0.4), &catalog).unwrap();
        assert_eq!(find(&g, "b").value, Some(0.4));
        set_held(&mut g, &c, "b", None, &catalog).unwrap();
        assert_eq!(find(&g, "b").value, None, "released back to its default");
        assert!(set_held(&mut g, &c, "h", Some(400.0), &catalog).unwrap_err().contains("outside"));
    }

    #[test]
    fn effects_hold_one_amount_and_forge_holds_none() {
        let catalog = catalog();
        let zoom = &kind(&catalog, "fx.zoom").params;
        let spec = &metal_schema::EFFECTS_FLOATS[metal_schema::effect_index("zoom").unwrap()];
        assert_eq!(zoom.len(), 1);
        assert_eq!((zoom[0].name.as_str(), zoom[0].min, zoom[0].max), ("amount", spec.min, spec.max));
        assert_eq!(kind(&catalog, "fx.text.wave").params[0].default, 1.0);
        let forge: Vec<&str> = kind(&catalog, crate::FORGE_EFFECT_KIND).params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(forge, FORGE_FX_PARAMS, "fx.forge has no amount, only pinnable ingest columns");
        for other in ["audio.live", "output.device", "mask", "script.py", "still", "router"] {
            assert!(kind(&catalog, other).params.is_empty(), "{other} exposes nothing on its own");
        }
    }

    /// 2026-10-01: `reel.batch` grew `interval`/`unit`/`order`/`seed`
    /// (appended, rule 2). A batch saved before them loads byte-for-byte the
    /// same, holds none of them, and a held one lands under its own key.
    #[test]
    fn an_old_reel_batch_loads_unchanged_and_holds_its_new_floats_under_own_keys() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let b = g.add_node("reel", crate::REEL_BATCH_KIND, [0.0, 0.0]);
        let text = g.to_json();
        let back = Graph::from_json(&text).unwrap();
        assert_eq!(back.to_json(), text, "an old batch round-trips unchanged");
        let kind = catalog.iter().find(|k| k.kind == crate::REEL_BATCH_KIND).unwrap();
        let names: Vec<&str> = kind.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["interval", "unit", "order", "seed"], "appended in this order");
        assert!(node_params(back.node(&b).unwrap(), Some(kind)).iter().all(|p| p.value.is_none()));
        set_held(&mut g, &b, "order", Some(1.0), &catalog).unwrap();
        assert_eq!(config_json(g.node(&b).unwrap())["order"], json!(1.0));
        assert_eq!(find_param(g.node(&b).unwrap(), Some(kind), "order").unwrap().value, Some(1.0));
    }

    #[test]
    fn held_values_are_read_off_each_kinds_own_config() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let fx = g.add_node("fx", "fx.vignette", [0.0, 0.0]);
        let text = g.add_node("fx", "fx.text.wave", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 0.0]);
        let find = |g: &Graph, id: &str, name: &str| {
            let n = g.node(id).unwrap();
            find_param(n, catalog.iter().find(|k| k.kind == n.kind), name).unwrap()
        };
        assert_eq!(find(&g, &fx, "amount").value, Some(0.0), "an effect always holds its fixed amount");
        assert_eq!(find(&g, &text, "amount").value, None);
        assert_eq!(find(&g, &gen, "scale").value, None, "an unheld generator float is left to the drive");

        with_config(&mut g, &fx, json!({ "fixed": 0.4 }));
        with_config(&mut g, &text, json!({ "amount": 0.25 }));
        with_config(&mut g, &gen, json!({ "floats": { "scale": 0.5 } }));
        with_config(
            &mut g,
            &script,
            json!({ "code": "", "floats": [{ "name": "gain", "min": 0.0, "max": 2.0, "default": 1.0, "value": 1.5 }] }),
        );
        assert_eq!(find(&g, &fx, "amount").value, Some(0.4));
        assert_eq!(find(&g, &text, "amount").value, Some(0.25));
        assert_eq!(find(&g, &gen, "scale").value, Some(0.5));
        let gain = find(&g, &script, "gain");
        assert_eq!((gain.min, gain.max, gain.default, gain.value, gain.index), (0.0, 2.0, 1.0, Some(1.5), Some(0)));
    }

    #[test]
    fn variables_per_kind() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [0.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 0.0]);
        let vars = |g: &Graph, id: &str, live: Option<usize>| {
            let n = g.node(id).unwrap();
            node_vars(n, catalog.iter().find(|k| k.kind == n.kind), live)
        };

        let a = vars(&g, &audio, None);
        assert_eq!(a.len(), metal_schema::DIGEST_COLUMNS.len() + REPEAT_VARS.len());
        assert_eq!((a[2].name.as_str(), a[2].port_ref(), a[2].index), ("onset", "control.onset".to_string(), Some(2)));

        let v = vars(&g, &gen, None);
        assert_eq!((v[0].kind, v[0].port_ref()), (VarKind::Image, "image".to_string()), "the render first");
        let row: Vec<&Var> = v.iter().filter(|v| v.port == "row").collect();
        let floats = kind(&catalog, "gen.turing").params.iter().filter(|p| p.kind == ParamKind::Float).count();
        assert_eq!(row.len(), floats, "a generator's row offers its float params");

        let f = vars(&g, &fx, None);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, VarKind::Image);

        assert!(vars(&g, &script, None).is_empty(), "no declaration, not running: nothing yet");
        assert_eq!(
            vars(&g, &script, Some(2)).iter().map(|v| v.name.as_str()).collect::<Vec<_>>(),
            ["out0", "out1"],
            "live-sized from the row it answered"
        );
        with_config(&mut g, &script, json!({ "code": "", "outputs": ["side", { "name": "width", "max": 2.0 }] }));
        let s = vars(&g, &script, Some(3));
        assert_eq!(s.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["side", "width", "out2"]);
        assert_eq!((s[1].max, s[1].index), (2.0, Some(1)));
    }

    /// An `audio.*` `control` offers the two Looper variables after the
    /// sixteen digest columns, `0..1` — appended, so a column keeps its index.
    #[test]
    fn an_audio_node_offers_the_two_repeat_variables() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let vars = node_vars(g.node(&audio).unwrap(), catalog.iter().find(|k| k.kind == "audio.live"), None);

        let locked = vars.iter().find(|v| v.name == "repeat.locked").expect("repeat.locked is offered");
        let phase = vars.iter().find(|v| v.name == "repeat.phase").expect("repeat.phase is offered");
        assert_eq!((locked.port.as_str(), locked.index), ("control", Some(metal_schema::DIGEST_COLUMNS.len())));
        assert_eq!((phase.port.as_str(), phase.index), ("control", Some(metal_schema::DIGEST_COLUMNS.len() + 1)));
        assert_eq!((locked.min, locked.max, phase.min, phase.max), (0.0, 1.0, 0.0, 1.0));
        assert_eq!(locked.port_ref(), "control.repeat.locked");
        assert_eq!(phase.port_ref(), "control.repeat.phase");

        for (i, name) in metal_schema::DIGEST_COLUMNS.iter().enumerate() {
            assert_eq!((vars[i].name.as_str(), vars[i].index), (*name, Some(i)), "the digest columns still come first");
        }

        let kind = catalog.iter().find(|k| k.kind == "audio.live");
        let resolved = resolve_var(g.node(&audio).unwrap(), kind, "control.repeat.phase").expect("resolve_var derives from node_vars");
        assert_eq!(resolved.index, Some(metal_schema::DIGEST_COLUMNS.len() + 1));
    }

    /// Every voice and tone column is a `control.*` variable, at the row slot
    /// the digester writes it to — so `control.sing` or `control.tone_conf` can
    /// be bound without a second declaration anywhere.
    #[test]
    fn control_variables_include_every_voice_and_tone_column() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let vars = node_vars(g.node(&audio).unwrap(), catalog.iter().find(|k| k.kind == "audio.live"), None);

        assert_eq!(
            metal_schema::DIGEST_COLUMNS.len(),
            metal_schema::MUSIC + metal_schema::VOICE + metal_schema::TONE
        );
        for (j, name) in metal_schema::VOICE_COLUMNS.iter().enumerate() {
            let i = metal_schema::MUSIC + j;
            let v = vars
                .iter()
                .find(|v| v.name == *name)
                .unwrap_or_else(|| panic!("{name} is not offered"));
            assert_eq!((v.port.as_str(), v.index), ("control", Some(i)), "{name}");
            assert_eq!(v.port_ref(), format!("control.{name}"));
            assert_eq!((v.min, v.max), (0.0, 1.0));
        }
        // The tone block is appended after the voice block, at the same index
        // the row carries it — so a binding to `control.tone_conf` works.
        for (j, name) in metal_schema::TONE_COLUMNS.iter().enumerate() {
            let i = metal_schema::MUSIC + metal_schema::VOICE + j;
            let v = vars
                .iter()
                .find(|v| v.name == *name)
                .unwrap_or_else(|| panic!("{name} is not offered"));
            assert_eq!((v.port.as_str(), v.index), ("control", Some(i)), "{name}");
            assert_eq!(v.port_ref(), format!("control.{name}"));
            assert_eq!((v.min, v.max), (0.0, 1.0));
        }
        // The Looper's two floats sit after the voice block, as before.
        let locked = vars.iter().find(|v| v.name == "repeat.locked").expect("repeat.locked");
        assert_eq!(locked.index, Some(metal_schema::FEATURES));
    }

    #[test]
    fn resolve_var_accepts_what_a_port_offers_and_refuses_the_rest() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let script = g.add_node("script", "script.js", [0.0, 0.0]);
        let r = |g: &Graph, id: &str, port: &str| {
            let n = g.node(id).unwrap();
            resolve_var(n, catalog.iter().find(|k| k.kind == n.kind), port)
        };
        assert_eq!(r(&g, &audio, "control.chroma C#").unwrap().index, Some(5));
        assert!(r(&g, &audio, "control.nope").is_err());
        assert!(r(&g, &audio, "control").is_err(), "a whole port is not a variable");
        assert!(r(&g, &gen, "row.scale").is_ok());
        assert!(r(&g, &gen, "image.image").is_err(), "a picture cannot drive a float");
        assert!(r(&g, &gen, "row.sig_level").is_err(), "the shared signal block is not offered");
        assert_eq!(r(&g, &script, "control.out7").unwrap().index, Some(7), "live-sized: any outN");
        assert!(r(&g, &script, "control.out07").is_err(), "only the canonical spelling");
        with_config(&mut g, &script, json!({ "code": "", "outputs": ["x"] }));
        assert!(r(&g, &script, "control.x").is_ok());
        let err = r(&g, &script, "control.out0").unwrap_err();
        assert!(err.contains("`x`"), "out0 is called x here: {err}");
        assert_eq!(r(&g, &script, "control.out1").unwrap().index, Some(1));
    }

    #[test]
    fn upstream_is_transitive_nearest_first_and_cycle_safe() {
        let mut g = Graph::new("g", "t", Domain::Live);
        let audio = g.add_node("audio", "audio.live", [0.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let fx = g.add_node("fx", "fx.shake", [0.0, 0.0]);
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&gen, "control"));
        g.add_edge(Endpoint::new(&audio, "control"), Endpoint::new(&script, "control"));
        g.add_edge(Endpoint::new(&gen, "image"), Endpoint::new(&fx, "image"));
        g.add_edge(Endpoint::new(&script, "control.out0"), Endpoint::new(&fx, "param:amount"));
        assert_eq!(upstream(&g, &fx), vec![(gen.clone(), 1), (script.clone(), 1), (audio.clone(), 2)]);
        assert_eq!(downstream(&g, &audio), vec![(gen.clone(), 1), (script.clone(), 1), (fx.clone(), 2)]);
        // A cycle (lint refuses it, a hand-edited file may still carry one).
        g.add_edge(Endpoint::new(&fx, "image"), Endpoint::new(&gen, "control"));
        let up = upstream(&g, &fx);
        assert!(!up.iter().any(|(id, _)| *id == fx), "the node itself is never its own source");
        assert_eq!(up.len(), 3);
    }

    #[test]
    fn set_held_writes_each_kinds_own_slot_and_refuses_what_is_not_there() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let fx = g.add_node("fx", "fx.zoom", [0.0, 0.0]);
        let gen = g.add_node("gen", "gen.turing", [0.0, 0.0]);
        let script = g.add_node("script", "script.py", [0.0, 0.0]);
        let out = g.add_node("out", "output.device", [0.0, 0.0]);
        with_config(&mut g, &script, json!({ "code": "", "floats": [{ "name": "gain", "min": 0.0, "max": 1.0, "default": 0.2 }] }));

        set_held(&mut g, &fx, "amount", Some(-0.5), &catalog).unwrap();
        assert_eq!(crate::effects::state_of(&g, &fx), metal_schema::EffectState::Fixed(-0.5));
        // Turing's `escala` spans 1..12 in the manifest.
        set_held(&mut g, &gen, "scale", Some(3.0), &catalog).unwrap();
        assert_eq!(gen_held(g.node(&gen).unwrap()).get("scale"), Some(&3.0));
        assert!(set_held(&mut g, &gen, "scale", Some(0.3), &catalog).unwrap_err().contains("outside"));
        set_held(&mut g, &script, "gain", Some(0.9), &catalog).unwrap();
        assert_eq!(g.node(&script).unwrap().config.get("floats").unwrap()[0]["value"], json!(0.9_f32));

        assert!(set_held(&mut g, &fx, "amount", Some(3.0), &catalog).unwrap_err().contains("outside"));
        assert!(set_held(&mut g, &gen, "nope", Some(0.1), &catalog).is_err());
        assert!(set_held(&mut g, &out, "amount", Some(0.1), &catalog).is_err());
        let colour = kind(&catalog, "gen.turing").params.iter().find(|p| p.kind == ParamKind::Colour).unwrap().name.clone();
        assert!(set_held(&mut g, &gen, &colour, Some(0.1), &catalog).unwrap_err().contains("colour"));

        // Releasing hands each one back to its default behaviour.
        set_held(&mut g, &gen, "scale", None, &catalog).unwrap();
        assert!(gen_held(g.node(&gen).unwrap()).is_empty(), "back to the digest drive");
        set_held(&mut g, &script, "gain", None, &catalog).unwrap();
        assert_eq!(g.node(&script).unwrap().config.get("floats").unwrap()[0].get("value"), None);
        set_held(&mut g, &fx, "amount", None, &catalog).unwrap();
        assert_eq!(crate::effects::state_of(&g, &fx), metal_schema::EffectState::Fixed(0.0));
    }

    /// B: the two media effects declare their floats (and `fx.particles` its
    /// `dust` colour slot), `fx.image-pop` carries the `select` input, and
    /// `set_held` writes and releases each float under its own config key.
    #[test]
    fn the_media_effects_declare_their_floats_and_hold_them_under_their_own_keys() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let p = g.add_node("fx", "fx.particles", [0.0, 0.0]);
        let ip = g.add_node("fx", "fx.image-pop", [1.0, 0.0]);

        let particles = kind(&catalog, "fx.particles");
        assert!(particles.params.iter().any(|x| x.name == "count" && x.kind == ParamKind::Float));
        assert!(particles.params.iter().any(|x| x.name == "dust" && x.kind == ParamKind::Colour));
        let image_pop = kind(&catalog, "fx.image-pop");
        assert!(image_pop.params.iter().any(|x| x.name == "concurrent"));
        assert!(
            image_pop.inputs.iter().any(|port| port.name == "select"),
            "fx.image-pop must declare the `select` port for ai.curator"
        );

        set_held(&mut g, &p, "count", Some(0.7), &catalog).unwrap();
        set_held(&mut g, &ip, "concurrent", Some(12.0), &catalog).unwrap();
        let pv = node_params(g.node(&p).unwrap(), Some(particles));
        assert_eq!(pv.iter().find(|x| x.name == "count").unwrap().value, Some(0.7));
        let iv = node_params(g.node(&ip).unwrap(), Some(image_pop));
        assert_eq!(iv.iter().find(|x| x.name == "concurrent").unwrap().value, Some(12.0));

        assert!(set_held(&mut g, &ip, "concurrent", Some(99.0), &catalog).unwrap_err().contains("outside"));
        set_held(&mut g, &p, "count", None, &catalog).unwrap();
        let released = node_params(g.node(&p).unwrap(), Some(particles));
        assert_eq!(released.iter().find(|x| x.name == "count").unwrap().value, None);
    }

    /// A gen parameter written under a Spanish alias is read back: the newest
    /// write wins over a stale canonical value, and one key holds it.
    #[test]
    fn a_gen_param_written_under_an_alias_is_the_one_read_back() {
        let catalog = catalog();
        let mut g = Graph::new("g", "t", Domain::Live);
        let o = g.add_node("gen", "gen.oracle", [0.0, 0.0]);
        let oracle = kind(&catalog, "gen.oracle");
        set_held(&mut g, &o, "size", Some(1.0), &catalog).unwrap();
        set_held(&mut g, &o, "tamano", Some(3.0), &catalog).unwrap();
        let v = node_params(g.node(&o).unwrap(), Some(oracle));
        assert_eq!(v.iter().find(|x| x.name == "size").unwrap().value, Some(3.0));
        let held = config_json(g.node(&o).unwrap())["floats"].clone();
        assert_eq!(held.as_object().unwrap().len(), 1, "one key per parameter: {held}");
    }

    /// F1: a `venue.fixture` node declares every config key with a default,
    /// and the three numeric ones are the catalog params — computable from the
    /// catalog, not a hand-written count (rule 7).
    #[test]
    fn a_venue_fixture_node_declares_every_config_key_with_a_default() {
        let defaults = venue_fixture_default_config();
        let obj = defaults.as_object().expect("a JSON object");
        assert_eq!(obj.len(), VENUE_FIXTURE_CONFIG_KEYS.len(), "one default per key");
        for key in VENUE_FIXTURE_CONFIG_KEYS {
            assert!(obj.contains_key(*key), "{key} has no default");
        }
        // The pointer is a pair of ids, empty until an operator points it at a
        // row; the colour source defaults to today's behaviour (decision 3).
        assert_eq!(defaults["venue"], serde_json::json!(""));
        assert_eq!(defaults["fixture"], serde_json::json!(""));
        assert_eq!(defaults["colour_from"], serde_json::json!("frame"));
        assert!(VENUE_FIXTURE_COLOUR_FROM.contains(&"frame"));
        assert!(VENUE_FIXTURE_SPREAD.contains(&"slots"));
        assert!(VENUE_FIXTURE_HUE_LOCK.contains(&"free"));

        let catalog = catalog();
        let k = kind(&catalog, crate::VENUE_FIXTURE_KIND);
        assert_eq!(k.params.len(), 3, "pick, offset and priority");
        for p in &k.params {
            assert_eq!(
                p.default,
                defaults[&p.name].as_f64().unwrap() as f32,
                "{}'s default comes from the one config list",
                p.name
            );
        }
    }
}

/// A P-A kind's own floats ([`crate::parity::floats_of`]) as params.
fn parity_params(kind: &str) -> Vec<Param> {
    crate::parity::floats_of(kind)
        .unwrap_or_default()
        .iter()
        .map(|&(name, min, max, default)| Param::float(name, min, max, default, None))
        .collect()
}

/// A `source.*` kind's own floats (M1, [`crate::source::floats_of`]) as params.
fn source_params(kind: &str) -> Vec<Param> {
    crate::source::floats_of(kind)
        .unwrap_or_default()
        .iter()
        .map(|&(name, min, max, default)| Param::float(name, min, max, default, None))
        .collect()
}
