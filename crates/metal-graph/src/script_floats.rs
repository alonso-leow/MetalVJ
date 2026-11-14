//! `S.AL.3`: the one validated writer for a `script.*` node's live float
//! values — `Node.config`'s `floats[i].value`, distinct from redeclaring a
//! float's own `name`/`min`/`max`/`default` (a heavier op with its own
//! shape: the *whole array*, which is what `set_params`'s `floats` key has
//! always meant before this — see `crates/metal-agent/src/harness.rs`'s
//! `set_params` and `data/agent/shots.json`'s `script-node-body-side-mask`).
//! Turning the knob in real time is a *narrower* write: `{"<name>": value}`,
//! an object rather than an array, so the two shapes tell each other apart
//! at the door without a second field name to remember.
//!
//! **Why this lives in `metal-graph` and not `metal-perform`.** Both
//! `metal_agent::harness::Harness::set_params` (the AI's route) and
//! `metal-dash`'s `POST /graph/node {op:"config"}` merge (the human panel's
//! route — `crate::call::graph::graph_node` in `metal-dash`) need to call the
//! *same* validated writer, per regla 4's "si no, dos": a value one route
//! would refuse and the other would silently accept is drift. `metal-agent`
//! depends on `metal-graph` but not on `metal-perform` (its harness works over
//! `serde_json::Value`, never `metal_perform::script::ScriptFloat`'s typed
//! shape), so `metal-graph` — the one domain crate both already build on — is
//! where a function reachable from both has to live. It only ever sees the
//! node's config as JSON, the same generic shape `metal_perform::script`'s own
//! `Node.config` reads live off every frame.

use serde_json::{json, Map, Value};

/// Applies `{"<float name>": <value>, ...}` onto `config`'s declared
/// `floats` array, writing only `value` for exactly the named entries —
/// `name`/`min`/`max`/`default` and every float not named in `patch` are
/// untouched, and the array is never reordered, grown or shrunk.
///
/// Refuses the **whole** patch, before writing anything, on the first
/// problem it finds: a name `config`'s `floats` does not declare, or a
/// value outside that float's own `[min, max]`. Never silently clamps on
/// write — clamping is read-time defense in depth
/// (`metal_perform::script::ScriptConfig::values`), not something this
/// function does on the caller's behalf, so a caller asking for something
/// out of range finds out rather than getting a silently different number.
pub fn apply_floats_patch(config: &mut Value, patch: &Map<String, Value>) -> Result<(), String> {
    let declared: Vec<Value> = config.get("floats").and_then(Value::as_array).cloned().unwrap_or_default();

    // Validate every entry first — a patch that fails partway through must
    // never leave some floats written and others refused.
    let mut resolved: Vec<(usize, f32)> = Vec::with_capacity(patch.len());
    for (name, v) in patch {
        let value = v.as_f64().ok_or_else(|| format!("float {name:?}: value must be a number"))? as f32;
        let idx = declared
            .iter()
            .position(|f| f.get("name").and_then(Value::as_str) == Some(name.as_str()))
            .ok_or_else(|| format!("no such float: {name:?}"))?;
        let min = declared[idx].get("min").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        let max = declared[idx].get("max").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        if value < min || value > max {
            return Err(format!("float {name:?}: {value} is outside its declared [{min}, {max}]"));
        }
        resolved.push((idx, value));
    }

    let mut declared = declared;
    for (idx, value) in resolved {
        declared[idx]["value"] = json!(value);
    }
    config["floats"] = Value::Array(declared);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(floats: Value) -> Value {
        json!({ "code": "", "floats": floats })
    }

    #[test]
    fn writes_the_named_floats_value_and_leaves_the_rest_of_its_declaration_alone() {
        let mut cfg = config(json!([
            { "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 },
            { "name": "height", "min": 0.0, "max": 2.0, "default": 0.5 },
        ]));
        let mut patch = Map::new();
        patch.insert("width".to_string(), json!(0.7));
        apply_floats_patch(&mut cfg, &patch).unwrap();

        // `0.7` round-trips through an `f32` on the way in (the write
        // side's own precision, matching `ScriptFloat::value`'s type) —
        // compare against `0.7_f32`'s own promotion to JSON, not the exact
        // `f64` literal, or this assertion would fail on the same rounding
        // every float write makes.
        assert_eq!(cfg["floats"][0]["value"], json!(0.7_f32));
        assert_eq!(cfg["floats"][0]["default"], json!(0.2), "default untouched");
        assert_eq!(cfg["floats"][1].get("value"), None, "an unpatched float gets no value field");
    }

    #[test]
    fn refuses_an_unknown_float_name() {
        let mut cfg = config(json!([{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 }]));
        let mut patch = Map::new();
        patch.insert("nope".to_string(), json!(0.5));
        let err = apply_floats_patch(&mut cfg, &patch).unwrap_err();
        assert!(err.contains("no such float"), "{err}");
        assert_eq!(cfg["floats"][0].get("value"), None, "a refused patch must not write anything");
    }

    #[test]
    fn refuses_a_value_outside_the_declared_range_without_clamping_it() {
        let mut cfg = config(json!([{ "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 }]));
        let mut patch = Map::new();
        patch.insert("width".to_string(), json!(5.0));
        let err = apply_floats_patch(&mut cfg, &patch).unwrap_err();
        assert!(err.contains("outside"), "{err}");
        assert_eq!(cfg["floats"][0].get("value"), None, "a refused patch must not write anything");
    }

    /// A patch touching two floats, one bad, refuses the whole thing —
    /// never a half-applied write.
    #[test]
    fn a_patch_with_one_bad_entry_writes_nothing_at_all() {
        let mut cfg = config(json!([
            { "name": "width", "min": 0.0, "max": 1.0, "default": 0.2 },
            { "name": "height", "min": 0.0, "max": 1.0, "default": 0.2 },
        ]));
        let mut patch = Map::new();
        patch.insert("width".to_string(), json!(0.9));
        patch.insert("height".to_string(), json!(9.0));
        assert!(apply_floats_patch(&mut cfg, &patch).is_err());
        assert_eq!(cfg["floats"][0].get("value"), None, "width must not have been written either");
    }
}
