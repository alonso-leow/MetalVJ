//! `fx.warp` on the engine side (the loops design notes T12): reading the node, and the
//! CPU path the stand-in painter (and a GPU that refuses the pass) uses.
//!
//! The notation — a coordinate mesh of control-point displacements,
//! Catmull-Rom interpolated into a field — is `metal_schema::warp`'s module
//! doc; the GPU pass is `metal_render::shared::warp`. Both evaluate the same
//! field. Mosh needs the node's previous output, so each path keeps a
//! history keyed by [`history_key`]: node id *and* frame size, because one
//! graph renders on several devices and a preview's smaller frame must not
//! reset the wall's history every other frame.

use std::collections::HashMap;
use std::sync::Mutex;

use metal_schema::warp::Warp;

/// The node kind.
pub const FX_WARP_KIND: &str = metal_graph::WARP_EFFECT_KIND;

/// The warp a node's config describes (each param under its own key).
pub fn warp_of(node: &metal_graph::Node) -> Warp {
    let cfg: serde_json::Value = serde_json::from_str(&node.config.to_json_str()).unwrap_or_default();
    Warp::from_config(&cfg)
}

/// What the history of a warp node at a frame size is filed under.
pub fn history_key(node: &str, w: u32, h: u32) -> String {
    format!("{node}@{w}x{h}")
}

static HISTORY: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

/// The CPU warp: `metal_schema::warp::apply_cpu` with this process's history.
pub fn apply_cpu(key: &str, warp: &Warp, frame: &(Vec<u8>, u32, u32)) -> Vec<u8> {
    let mut guard = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    let history = guard.get_or_insert_with(HashMap::new);
    let out = metal_schema::warp::apply_cpu(warp, &frame.0, history.get(key).map(Vec::as_slice), frame.1, frame.2);
    history.insert(key.to_string(), out.clone());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_reads_its_params_and_amount_zero_passes_through() {
        let mut n = metal_graph::Node::new("w", FX_WARP_KIND, [0.0, 0.0]);
        n.config = metal_graph::Config::from_json_str(r#"{"amount":0.4,"mode":0.7}"#).unwrap();
        let w = warp_of(&n);
        assert_eq!((w.amount, w.pattern()), (0.4, 4));
        let frame: (Vec<u8>, u32, u32) = ((0..16 * 9 * 4).map(|i| (i % 251) as u8).collect(), 16, 9);
        let still = Warp::default();
        assert_eq!(apply_cpu("test-still", &still, &frame), frame.0);
        assert_ne!(apply_cpu("test-moved", &w, &frame), frame.0);
    }
}
