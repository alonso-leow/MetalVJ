//! The float parameters a controller editor plays, read off `GET /graph/io`'s
//! `params`: the catalog's own floats with their `min`/`max`, each with its
//! live value (else its default, else `min`).
//!
//! Nothing here paints or talks to the door: the editor reuses the grid's one
//! fader (`builtins::fader`) and posts `POST /graph/node {op:"param"}` itself.
//! [`floats`] is the reading both the node list and the editor share.

use serde_json::Value;

/// One float parameter a controller can play.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Float {
    pub name: String,
    pub min: f32,
    pub max: f32,
    pub value: f32,
}

/// The float params of a `GET /graph/io` reply, their live value (else their
/// default) as the starting point. A float with no declared range reads
/// `0..=1` rather than being dropped — a generator float whose range the
/// manifest left off still appears and is playable.
pub(crate) fn floats(io: &Value) -> Vec<Float> {
    io["params"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["kind"].as_str().unwrap_or("float") == "float")
        .filter_map(|p| {
            let min = p["min"].as_f64().unwrap_or(0.0) as f32;
            let max = p["max"].as_f64().unwrap_or(1.0) as f32;
            let value = p["live"].as_f64().or(p["default"].as_f64()).unwrap_or(min as f64) as f32;
            Some(Float { name: p["name"].as_str()?.to_string(), min, max, value })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The floats read the node's float params with their range, the live
    /// value when the engine has one, else the default; non-floats are left out.
    #[test]
    fn dials_are_the_nodes_float_params() {
        let io = json!({ "params": [
            { "name": "scale", "min": 1.0, "max": 12.0, "default": 1.0, "kind": "float", "live": 3.0 },
            { "name": "amount", "min": 0.0, "max": 1.0, "default": 0.5, "kind": "float", "live": null },
            { "name": "mode", "min": 0.0, "max": 3.0, "default": 0.0, "kind": "enum" }
        ]});
        let f = floats(&io);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], Float { name: "scale".into(), min: 1.0, max: 12.0, value: 3.0 });
        assert_eq!(f[1].value, 0.5);
    }

    /// A float the manifest left without a range is not dropped: it reads
    /// `0..=1`, so a generator float always appears and is playable.
    #[test]
    fn a_float_without_a_declared_range_still_appears() {
        let io = json!({ "params": [{ "name": "scale", "default": 0.5, "kind": "float" }] });
        let f = floats(&io);
        assert_eq!(f, vec![Float { name: "scale".into(), min: 0.0, max: 1.0, value: 0.5 }]);
    }
}
