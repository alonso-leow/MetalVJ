//! M1 of the show-control lote (plan "Value sources"): the `source.*` value
//! nodes — a pure CPU family of **control** sources at [`crate::Stage::Source`].
//!
//! Four kinds, all plainly non-pictures:
//!
//! - `source.osc`: a wave (config `wave`: `sine`/`square`/`triangle`/`saw`)
//!   at `freq`, `phase`, `amp`, `offset`, either on the song clock or locked
//!   to a beat division (`sync`), jittered by `seed`;
//! - `source.const`: a number (`value`) or a named constant (config
//!   `constant`: `pi`/`e`/`tau`);
//! - `source.random`: a pure `mix32`/`unit` stream on
//!   `(seed, floor(t·rate), channel)` — never `rand` — held when `rate` is 0,
//!   shaped by `smooth`, `min`/`max` and `distribution`, `per` channels;
//! - `source.noise`: M2 of the lote — **value noise** (quintic, the same
//!   integer-lattice hash as `shaders/shared.wgsl`'s `quintic_noise`/
//!   `lattice_fbm`, reimplemented in Rust, no GPU pass) summed over
//!   `octaves`, mapped into `offset ± amp` and held when `rate` is 0, sampled
//!   along `axis` (`t`/`x`/`y`/`z`, config `axis`).
//!
//! Each offers its value(s) on the `control` port as `out0..`, exactly the row
//! a `script.*` node answers, so a `param:<name>` binding reads them the same
//! way ([`crate::params`]). They are **not** models: `registry::all_models()`
//! is untouched (rule 1). `gen.ramp` is the sibling precedent for a source
//! that is not a model; `metal_perform::source` is the pure reader.
//!
//! # Ranges
//!
//! A binding normalises a variable by its declared range ([`crate::params::Var`]),
//! so a source node's output range is derived from its own config —
//! `offset ± amp` for the oscillator and the value noise, `min..max` (ordered)
//! for `source.random`, the declared `value` range for the constant — never a
//! fixed `0..1` that would clip a swinging wave or a noise field.

use crate::{Node, NodeFamily, NodeKind, Port, PortType};

/// One float parameter: `(name, min, max, default)` — the same shape
/// [`crate::parity::FloatDef`] uses.
pub type FloatDef = crate::parity::FloatDef;

/// The oscillator kind.
pub const OSC_KIND: &str = "source.osc";
/// The constant kind.
pub const CONST_KIND: &str = "source.const";
/// The random-stream kind.
pub const RANDOM_KIND: &str = "source.random";
/// The value-noise kind (M2).
pub const NOISE_KIND: &str = "source.noise";

/// The four kinds, in catalog order.
pub const KINDS: &[&str] = &[OSC_KIND, CONST_KIND, RANDOM_KIND, NOISE_KIND];

/// The waves `source.osc`'s config `wave` selects, in order.
pub const OSC_WAVES: &[&str] = &["sine", "square", "triangle", "saw"];

/// The axes `source.noise`'s config `axis` selects, in order: which lattice
/// dimension the clock walks. Index for index with `source.noise`'s `axis`
/// float (0 = time, 1 = x, 2 = y, 3 = z).
pub const NOISE_AXES: &[&str] = &["t", "x", "y", "z"];

/// The named constants `source.const`'s config `constant` selects (empty
/// `constant` = the node's own `value`).
pub const CONST_NAMES: &[&str] = &["pi", "e", "tau"];

/// The value of a named constant, or `None` for anything else.
pub fn named_constant(name: &str) -> Option<f32> {
    match name {
        "pi" => Some(std::f32::consts::PI),
        "e" => Some(std::f32::consts::E),
        "tau" => Some(std::f32::consts::TAU),
        _ => None,
    }
}

/// `source.osc`'s floats.
pub const OSC_FLOATS: &[FloatDef] = &[
    ("freq", 0.0, 40.0, 1.0),
    ("phase", 0.0, 1.0, 0.0),
    ("amp", 0.0, 10.0, 1.0),
    ("offset", -10.0, 10.0, 0.0),
    ("sync", 0.0, 8.0, 0.0),
    ("seed", 0.0, 1000.0, 0.0),
];

/// `source.const`'s floats.
pub const CONST_FLOATS: &[FloatDef] = &[("value", -10.0, 10.0, 0.0)];

/// `source.random`'s floats. `distribution`: 0 uniform, 1 triangular. `per`
/// is how many independent channels the node offers (`out0..out{per-1}`).
pub const RANDOM_FLOATS: &[FloatDef] = &[
    ("seed", 0.0, 1000.0, 0.0),
    ("rate", 0.0, 100.0, 4.0),
    ("smooth", 0.0, 0.99, 0.0),
    ("min", -10.0, 10.0, 0.0),
    ("max", -10.0, 10.0, 1.0),
    ("distribution", 0.0, 1.0, 0.0),
    ("per", 1.0, 32.0, 1.0),
];

/// `source.noise`'s floats (M2), appended in this order and never reordered
/// (rule 2). `rate` is cycles/s along the chosen `axis` (0 = held), `amp`
/// the swing either side of `offset`, `octaves` 1..=8 the fbm depth,
/// `lacunarity` the per-octave frequency step and `gain` its amplitude step
/// (`gain` 0 = one octave only). `axis` is `0..3` → `t`/`x`/`y`/`z`
/// ([`NOISE_AXES`]).
pub const NOISE_FLOATS: &[FloatDef] = &[
    ("seed", 0.0, 1000.0, 0.0),
    ("rate", 0.0, 100.0, 1.0),
    ("amp", 0.0, 10.0, 1.0),
    ("octaves", 1.0, 8.0, 1.0),
    ("lacunarity", 1.0, 8.0, 2.03),
    ("gain", 0.0, 1.0, 0.5),
    ("axis", 0.0, 3.0, 0.0),
    ("offset", -10.0, 10.0, 0.0),
];

/// The floats a `source.*` kind declares, `None` for every other kind.
pub fn floats_of(kind: &str) -> Option<&'static [FloatDef]> {
    Some(match kind {
        OSC_KIND => OSC_FLOATS,
        CONST_KIND => CONST_FLOATS,
        RANDOM_KIND => RANDOM_FLOATS,
        NOISE_KIND => NOISE_FLOATS,
        _ => return None,
    })
}

/// Whether `kind` is one of the four `source.*` value nodes — the one test
/// every reader of the family shares.
pub fn is_source_kind(kind: &str) -> bool {
    matches!(kind, OSC_KIND | CONST_KIND | RANDOM_KIND | NOISE_KIND)
}

/// The declared default of `name` on `kind`, or `None` when the kind does not
/// carry it.
pub fn default_of(kind: &str, name: &str) -> Option<f32> {
    floats_of(kind)
        .and_then(|fs| fs.iter().find(|f| f.0 == name))
        .map(|f| f.3)
}

/// `name`'s held value on `node`: its own config key when it holds one,
/// else the kind's declared default. The reading every source parameter
/// shares — a source holds its floats plainly, like `gen.ramp`.
pub fn held(node: &Node, name: &str) -> f32 {
    node.config
        .get(name)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
        .or_else(|| default_of(&node.kind, name))
        .unwrap_or(0.0)
}

/// How many values `node` offers on its `control` port: `per` for
/// `source.random` (clamped `1..=32`), one for the other three. Known from the
/// config, so a reader does not need a live frame to name the variables.
pub fn output_count(node: &Node) -> usize {
    if node.kind == RANDOM_KIND {
        held(node, "per").round().clamp(1.0, 32.0) as usize
    } else {
        1
    }
}

/// The range a bound consumer normalises `node`'s output against — see the
/// module doc: `offset ± amp` for the oscillator and the value noise, the
/// ordered `min..max` for `source.random`, and the declared `value` range for
/// the constant.
pub fn output_range(node: &Node) -> (f32, f32) {
    match node.kind.as_str() {
        OSC_KIND | NOISE_KIND => {
            let amp = held(node, "amp");
            let offset = held(node, "offset");
            (offset - amp, offset + amp)
        }
        CONST_KIND => CONST_FLOATS
            .first()
            .map(|f| (f.1, f.2))
            .unwrap_or((-10.0, 10.0)),
        RANDOM_KIND => {
            let lo = held(node, "min");
            let hi = held(node, "max");
            if lo <= hi {
                (lo, hi)
            } else {
                (hi, lo)
            }
        }
        _ => (0.0, 1.0),
    }
}

/// The `source.*` kinds the catalog appends, with their ports — one `control`
/// output and no input, a source of value like a `color` node. See
/// [`crate::NodeKind::catalog`].
pub(crate) fn kinds() -> Vec<NodeKind> {
    let mut out = Vec::with_capacity(KINDS.len());
    for (kind, label) in [
        (OSC_KIND, "Oscillator"),
        (CONST_KIND, "Constant"),
        (RANDOM_KIND, "Random"),
        (NOISE_KIND, "Noise"),
    ] {
        out.push(NodeKind::new(
            kind,
            NodeFamily::Math,
            label,
            vec![],
            vec![Port::new("control", PortType::Control)],
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::VarKind;
    use crate::Stage;

    fn catalog() -> Vec<NodeKind> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/schema.json");
        let text = metal_vfs::read_to_string(path).expect("data/schema.json readable");
        NodeKind::catalog(&metal_schema::Manifest::parse(&text).expect("schema.json parses"))
    }

    /// Every `source.*` kind is in the catalog, once, with exactly the floats
    /// this module declares — counted from the tables, never written down.
    #[test]
    fn every_source_kind_is_in_the_catalog_with_its_floats() {
        let cat = catalog();
        for kind in KINDS {
            let found: Vec<&NodeKind> = cat.iter().filter(|k| &k.kind == kind).collect();
            assert_eq!(found.len(), 1, "{kind} once in the catalog");
            let names: Vec<&str> = found[0].params.iter().map(|p| p.name.as_str()).collect();
            let want: Vec<&str> = floats_of(kind).unwrap().iter().map(|f| f.0).collect();
            assert_eq!(names, want, "{kind}'s params are its floats, in order");
            for (p, f) in found[0].params.iter().zip(floats_of(kind).unwrap()) {
                assert!(
                    p.min <= f.3 && f.3 <= p.max,
                    "{kind}.{} default inside its range",
                    p.name
                );
            }
        }
    }

    /// A source sits at `Stage::Source`, offers one `control` out and nothing
    /// else, needs no GPU and is reachable from the menu (the route the AI's
    /// `create_node`/`search_nodes` share, rule 6).
    #[test]
    fn the_source_family_is_three_sources_of_control() {
        let cat = catalog();
        for kind in KINDS {
            let k = cat.iter().find(|k| k.kind == *kind).unwrap();
            assert_eq!(k.family, NodeFamily::Math);
            assert_eq!(
                k.stage(),
                Stage::Source,
                "{kind} is where a value comes from"
            );
            assert!(
                k.menu,
                "{kind} must be reachable from the menu and the AI route"
            );
            assert!(k.inputs.is_empty(), "{kind} is a source: nothing feeds it");
            assert_eq!(k.outputs.len(), 1, "{kind} offers exactly one port");
            assert_eq!(
                (k.outputs[0].name.as_str(), k.outputs[0].ty),
                ("control", PortType::Control)
            );
            let c = k.capabilities();
            assert!(c.has(crate::Capability::ControlOut));
            assert!(
                !c.has(crate::Capability::ImageOut),
                "{kind} draws no picture"
            );
            assert!(!c.has(crate::Capability::NeedsGpu), "{kind} needs no GPU");
            assert!(
                !c.has(crate::Capability::NeedsAudio),
                "{kind} hears nothing"
            );
        }
    }

    /// The variables a node offers follow its config: `per` channels for the
    /// noise, one for the others, each a `Float` on `control`, numbered from 0.
    #[test]
    fn a_source_offers_one_control_variable_per_channel() {
        use crate::Config;
        let mut node = Node {
            id: "s".into(),
            kind: RANDOM_KIND.into(),
            pos: [0.0, 0.0],
            label: String::new(),
            bypass: false,
            solo: false,
            expanded: false,
            config: Config::default(),
            embedded_in: None,
        };
        assert_eq!(output_count(&node), 1, "per defaults to one");
        node.config = Config::from_json_str("{\"per\": 4}").unwrap();
        assert_eq!(output_count(&node), 4);
        let vars = crate::params::node_vars(&node, None, None);
        assert_eq!(vars.len(), 4);
        for (i, v) in vars.iter().enumerate() {
            assert_eq!(v.name, format!("out{i}"));
            assert_eq!(v.kind, VarKind::Float);
            assert_eq!(v.port, "control");
            assert_eq!(v.index, Some(i));
        }
    }

    /// The oscillator's output range follows `offset ± amp`, the noise's the
    /// ordered `min..max`.
    #[test]
    fn a_source_range_follows_its_config() {
        use crate::Config;
        let osc = Node {
            id: "o".into(),
            kind: OSC_KIND.into(),
            pos: [0.0, 0.0],
            label: String::new(),
            bypass: false,
            solo: false,
            expanded: false,
            config: Config::from_json_str("{\"amp\": 2.0, \"offset\": 1.0}").unwrap(),
            embedded_in: None,
        };
        assert_eq!(output_range(&osc), (-1.0, 3.0));
        let mut rand = osc.clone();
        rand.kind = RANDOM_KIND.into();
        rand.config = Config::from_json_str("{\"min\": 0.8, \"max\": 0.2}").unwrap();
        assert_eq!(output_range(&rand), (0.2, 0.8), "min/max are ordered");
    }
}
