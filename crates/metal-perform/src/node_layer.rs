//! `the design notes` §2.5 / plan B3 paso 4: the per-node config that
//! lets an authored `gen.*` node say something about *how* it composites —
//! blend, z, sector, mask, opacity — instead of leaving those five entirely
//! to the `Layer` the graph was authored against.
//!
//! **Everything here is optional, and that is the whole trick.** A `gen.*`
//! node with no `NodeLayerConfig` (every node written before this existed,
//! and every node an operator never opens this panel on) applies nothing,
//! so [`apply_to`] is a no-op and the layer's own field — the value the
//! legacy `outs`/`srcs` UI still writes — wins exactly as it always has.
//! That is the golden corpus (`metal_perform::engine`'s tests): this file
//! adds a way to *override*, never a new default.
//!
//! **`mode` (automix/collection, per `layers::DeviceMode`) is deliberately
//! not here.** `compile_device_plan` reads `stack.mode` to decide *which*
//! layers exist — automix synthesises its own pair, `Collection(id)` pulls
//! a whole different `Vec<Layer>` — before it ever asks whether an authored
//! graph exists. A `gen.*` node's config cannot reach back and change which
//! layers were resolved to build the very list it is a member of; doing so
//! would need a graph-shaped stand-in for "what a device shows", which is a
//! bigger step than this one. See `engine.rs`'s `compile_device_plan` and the
//! B3-paso-4 report for the rest of that reasoning.

use crate::layers::{Blend, Layer, Sector};
use serde::{Deserialize, Serialize};

/// What a `gen.*` node can pin, read straight out of [`metal_graph::Node::config`].
///
/// Every field `Option`, `#[serde(default)]`'d, so a config with none of
/// them (`{}`, or absent entirely — [`metal_graph::Config::is_empty`]) parses
/// to "override nothing".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NodeLayerConfig {
    #[serde(default)]
    pub blend: Option<Blend>,
    #[serde(default)]
    pub z: Option<i32>,
    #[serde(default)]
    pub sector: Option<Sector>,
    /// `Some(None)` is not representable in JSON the way this is written —
    /// this is "pin the mask to this contour id", not "pin no-mask". A
    /// node cannot force a layer's mask *off*, only choose one; the legacy
    /// UI can still clear it. That is a narrower surface than the layer
    /// field's own `Option<String>`, and an honest one: nothing in the ask
    /// asked for "force no mask", and adding it here for symmetry alone is
    /// exactly the over-building this step is supposed to avoid.
    #[serde(default)]
    pub mask: Option<String>,
    #[serde(default)]
    pub opacity: Option<f32>,
    /// A `forge` source's shader id — what `Paint::run` reads off the node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shader: Option<String>,
}

impl NodeLayerConfig {
    /// Reads whatever a node's free-form `config` holds as a
    /// `NodeLayerConfig`, treating anything that does not parse as one —
    /// an empty config, a `fx.*` node's own shape, garbage from a hand-edited
    /// file — as "no overrides" rather than a hard error. This runs on the
    /// render path; a malformed config must fall back, not blank the screen.
    pub fn from_node(node: &metal_graph::Node) -> NodeLayerConfig {
        if node.config.is_empty() {
            return NodeLayerConfig::default();
        }
        serde_json::from_str(&node.config.to_json_str()).unwrap_or_default()
    }

    pub fn to_config(&self) -> metal_graph::Config {
        // unwrap: `NodeLayerConfig` only ever contains types that already
        // round-trip through serde_json elsewhere in this crate (`Blend`,
        // `Sector`, `String`, numbers), so this cannot fail.
        let s = serde_json::to_string(self).expect("NodeLayerConfig always serializes");
        metal_graph::Config::from_json_str(&s).expect("just-serialized JSON always parses")
    }

    pub fn is_empty(&self) -> bool {
        *self == NodeLayerConfig::default()
    }

    /// Overrides `layer`'s own blend/z/sector/mask/opacity with whatever
    /// this config pins, leaving anything absent untouched.
    pub fn apply_to(&self, layer: &mut Layer) {
        if let Some(blend) = self.blend {
            layer.blend = blend;
        }
        if let Some(z) = self.z {
            layer.z = z;
        }
        if let Some(sector) = self.sector {
            layer.sector = sector.clamped();
        }
        if let Some(mask) = &self.mask {
            layer.mask = Some(mask.clone());
        }
        if let Some(opacity) = self.opacity {
            layer.opacity = opacity;
        }
        if let Some(shader) = &self.shader {
            layer.shader = Some(shader.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_round_trips_to_default() {
        let node = metal_graph::Node::new("g1", "gen.turing", [0.0, 0.0]);
        let cfg = NodeLayerConfig::from_node(&node);
        assert!(cfg.is_empty());
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = NodeLayerConfig {
            blend: Some(Blend::Add),
            z: Some(3),
            sector: Some(Sector {
                x: 0.1,
                y: 0.2,
                w: 0.3,
                h: 0.4,
            }),
            mask: Some("puerta".into()),
            opacity: Some(0.5),
            shader: None,
        };
        let mut node = metal_graph::Node::new("g1", "gen.turing", [0.0, 0.0]);
        node.config = cfg.to_config();
        let back = NodeLayerConfig::from_node(&node);
        assert_eq!(cfg, back);
    }

    #[test]
    fn apply_to_overrides_only_what_is_set() {
        let mut layer = Layer::new("l1", "uno", 0);
        layer.blend = Blend::Normal;
        layer.z = 0;
        layer.opacity = 1.0;
        let cfg = NodeLayerConfig {
            blend: Some(Blend::Screen),
            z: None,
            ..Default::default()
        };
        cfg.apply_to(&mut layer);
        assert_eq!(layer.blend, Blend::Screen);
        assert_eq!(layer.z, 0); // untouched
        assert_eq!(layer.opacity, 1.0); // untouched
    }
}
