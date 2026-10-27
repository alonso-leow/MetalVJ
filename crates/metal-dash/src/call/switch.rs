//! `S.AK`/`O.28`: `GET`/`POST /switch` — the grid's on/off buttons.
//!
//! `GET /switch` lists every saved switch with its current `on`, read fresh
//! off the graph each time (regla 4 — see `metal_perform::switch::Switch::state`).
//! `POST /switch` is `{op: "add"|"remove"|"set"|"on"|"off"}`: `add`/`remove`/
//! `set` edit `data/switches.json`; `on`/`off` apply a switch's `set`/`unset`
//! batch to its graph in one save (`metal_perform::switch::apply` +
//! `engine::save_graph`) — the same one-save discipline `POST /graph/node
//! {op:"bypass"}` now shares with this route (see `graph.rs`).

use serde_json::{json, Value};

use metal_perform::switch::{ButtonMode, Switch, SwitchEntry, SwitchKind};

use super::{CallError, Ctx, Params};

fn parse_kind(s: &str) -> Result<SwitchKind, CallError> {
    match s {
        "toggle" => Ok(SwitchKind::Toggle),
        "hold" => Ok(SwitchKind::Hold),
        "fixture" => Ok(SwitchKind::Fixture),
        "rig" => Ok(SwitchKind::Rig),
        other => Err(CallError::Bad(format!(
            "no such switch kind: {other} — expected toggle, hold, fixture or rig"
        ))),
    }
}

/// G3: the `hold`/`switch` mode a fixture/rig button carries. Absent is "leave
/// it" (a `None` field on `add`, no change on `set`); `null` clears it.
fn parse_mode(v: Option<&Value>) -> Result<Option<Option<ButtonMode>>, CallError> {
    match v {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => match s.as_str() {
            "switch" => Ok(Some(Some(ButtonMode::Switch))),
            "hold" => Ok(Some(Some(ButtonMode::Hold))),
            other => Err(CallError::Bad(format!(
                "no such button mode: {other} — expected hold or switch"
            ))),
        },
        Some(other) => Err(CallError::Bad(format!(
            "no such button mode: {other} — expected hold or switch"
        ))),
    }
}

/// The `fixture`/`group` a G1 button names, read off the body when present.
/// Both are optional (a node switch carries neither); an explicit `null`
/// clears the field on `set`, the same way `unset: null` does.
fn parse_opt_str(v: Option<&Value>) -> Option<Option<String>> {
    match v {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(s)) => Some(Some(s.clone())),
        Some(other) => Some(Some(other.to_string())),
    }
}

fn parse_entries(v: &Value) -> Result<Vec<SwitchEntry>, CallError> {
    let arr = v
        .as_array()
        .ok_or_else(|| CallError::Bad("expected an array of {node, on}".into()))?;
    arr.iter()
        .map(|e| {
            let node = e["node"]
                .as_str()
                .ok_or_else(|| CallError::Bad("each entry needs a `node`".into()))?
                .to_string();
            let on = e["on"]
                .as_bool()
                .ok_or_else(|| CallError::Bad("each entry needs `on` (bool)".into()))?;
            let param = e.get("param").and_then(Value::as_str).map(str::to_string);
            let value = e.get("value").and_then(Value::as_f64).map(|v| v as f32);
            Ok(SwitchEntry { node, on, param, value })
        })
        .collect()
}

/// `GET /switch`.
pub fn switch_get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    let list: Vec<Value> = ctx
        .state
        .switches
        .list
        .iter()
        .map(|sw| {
            let g = crate::load_graph(&sw.graph).ok();
            sw.describe(g.as_ref())
        })
        .collect();
    Ok(json!({ "switches": list }))
}

/// `POST /switch`.
pub fn switch_post(ctx: &mut Ctx, params: &Params) -> Result<Value, CallError> {
    let Some(body) = params.json() else {
        return Err(CallError::Bad("expected json".into()));
    };
    match body["op"].as_str().unwrap_or_default() {
        "add" => {
            let id = body["id"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which id".into()))?
                .to_string();
            if ctx.state.switches.find(&id).is_some() {
                return Err(CallError::Bad(format!("a switch already exists: {id}")));
            }
            let name = body["name"].as_str().unwrap_or(&id).to_string();
            let kind = parse_kind(body["kind"].as_str().unwrap_or("toggle"))?;
            let graph = body["graph"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which graph".into()))?
                .to_string();
            let set = parse_entries(&body["set"])?;
            let unset = match body.get("unset") {
                Some(Value::Null) | None => None,
                Some(v) => Some(parse_entries(v)?),
            };
            ctx.state.switches.list.push(Switch {
                id: id.clone(),
                name,
                kind,
                graph,
                set,
                unset,
                fixture: parse_opt_str(body.get("fixture")).flatten(),
                group: parse_opt_str(body.get("group")).flatten(),
                mode: parse_mode(body.get("mode"))?.flatten(),
            });
            ctx.state
                .switches
                .save()
                .map_err(|e| CallError::Internal(e.to_string()))?;
            Ok(json!({ "switch": id }))
        }
        "remove" => {
            let id = body["id"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which id".into()))?;
            if !ctx.state.switches.remove(id) {
                return Err(CallError::NotFound);
            }
            ctx.state
                .switches
                .save()
                .map_err(|e| CallError::Internal(e.to_string()))?;
            Ok(json!({ "removed": id }))
        }
        "set" => {
            let id = body["id"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which id".into()))?
                .to_string();
            let name = body["name"].as_str().map(str::to_string);
            let kind = body["kind"].as_str().map(parse_kind).transpose()?;
            let graph = body["graph"].as_str().map(str::to_string);
            let set = match body.get("set") {
                Some(v) => Some(parse_entries(v)?),
                None => None,
            };
            let unset = match body.get("unset") {
                Some(Value::Null) => Some(None),
                Some(v) => Some(Some(parse_entries(v)?)),
                None => None,
            };
            let fixture = parse_opt_str(body.get("fixture"));
            let group = parse_opt_str(body.get("group"));
            let mode = parse_mode(body.get("mode"))?;
            let sw = ctx
                .state
                .switches
                .find_mut(&id)
                .ok_or(CallError::NotFound)?;
            if let Some(name) = name {
                sw.name = name;
            }
            if let Some(kind) = kind {
                sw.kind = kind;
            }
            if let Some(graph) = graph {
                sw.graph = graph;
            }
            if let Some(set) = set {
                sw.set = set;
            }
            if let Some(unset) = unset {
                sw.unset = unset;
            }
            if let Some(fixture) = fixture {
                sw.fixture = fixture;
            }
            if let Some(group) = group {
                sw.group = group;
            }
            if let Some(mode) = mode {
                sw.mode = mode;
            }
            ctx.state
                .switches
                .save()
                .map_err(|e| CallError::Internal(e.to_string()))?;
            Ok(json!({ "switch": id }))
        }
        op @ ("on" | "off") => {
            let id = body["id"]
                .as_str()
                .ok_or_else(|| CallError::Bad("which switch".into()))?;
            let sw = ctx
                .state
                .switches
                .find(id)
                .cloned()
                .ok_or(CallError::NotFound)?;
            let entries = if op == "on" {
                sw.set.clone()
            } else {
                sw.off_entries()
            };
            let mut g = crate::load_graph(&sw.graph).map_err(CallError::Bad)?;
            // Parity P-E: the toggle instant, so the engine plays the node's
            // in/out transition (`metal_perform::transition`).
            let now = ctx.state.time();
            ctx.state.transitions.stamp(&g, &entries, now);
            metal_perform::switch::apply(&mut g, &entries);
            crate::save_graph(&g).map_err(CallError::Internal)?;
            Ok(json!({ "switch": id, "on": op == "on" }))
        }
        other => Err(CallError::Bad(format!("no such op: {other}"))),
    }
}

/// `GET /transitions`: every preset, re-read from disk (rule 4) so a file
/// dropped into `data/transitions/` shows without a restart.
pub fn transitions_get(ctx: &mut Ctx, _params: &Params) -> Result<Value, CallError> {
    ctx.state.transition_presets = metal_perform::transition::load_presets();
    let list: Vec<Value> = ctx
        .state
        .transition_presets
        .iter()
        .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
        .collect();
    Ok(json!({ "presets": list }))
}

// The route handlers above touch `output::data_root()` (via
// `Switches::save`/`crate::load_graph`/`crate::save_graph`), which — like
// every other handler in this module (`graph::graph_node` included) —
// defaults to this repo's real `data/`. The established way this crate
// tests such a handler is *not* calling it directly (see `graph.rs`'s own
// `mod tests`, which exercises `set_graph_device_mode_at` rather than
// `graph_node`); the parsing and mutation logic this route is a thin
// wrapper over is covered instead by `metal_perform::switch`'s own test module
// — `apply`, `Switch::state`, and `Switch::off_entries`, against graphs held
// entirely in memory. What *is* tested here is the parsing the properties
// panel's editor (`S.AK` point 5, `panels::properties::switches_section`)
// relies on: the bodies it posts are exactly what these parsers accept.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_accepts_the_two_kinds_the_editor_offers() {
        assert_eq!(parse_kind("toggle").unwrap(), SwitchKind::Toggle);
        assert_eq!(parse_kind("hold").unwrap(), SwitchKind::Hold);
        assert!(matches!(parse_kind("latch"), Err(CallError::Bad(_))));
    }

    /// G1: the two fixture-family kinds the "new button" dropdown offers
    /// parse to their own variants, so a `fixture`/`rig` button is a real
    /// saved switch rather than a `toggle` in disguise.
    #[test]
    fn parse_kind_accepts_the_fixture_and_rig_button_kinds() {
        assert_eq!(parse_kind("fixture").unwrap(), SwitchKind::Fixture);
        assert_eq!(parse_kind("rig").unwrap(), SwitchKind::Rig);
    }

    /// G3: the `hold`/`switch` mode parses to the two variants, absent is
    /// "leave it" and `null` clears it.
    #[test]
    fn parse_mode_reads_the_fixture_button_modes() {
        assert_eq!(parse_mode(None).unwrap(), None);
        assert_eq!(parse_mode(Some(&Value::Null)).unwrap(), Some(None));
        assert_eq!(parse_mode(Some(&json!("hold"))).unwrap(), Some(Some(ButtonMode::Hold)));
        assert_eq!(parse_mode(Some(&json!("switch"))).unwrap(), Some(Some(ButtonMode::Switch)));
        assert!(matches!(parse_mode(Some(&json!("latch"))), Err(CallError::Bad(_))));
    }

    /// The `fixture`/`group` fields a G1 body carries: absent is "leave it",
    /// `null` is "clear it", a string is the value.
    #[test]
    fn parse_opt_str_reads_the_fixture_family_targets() {
        assert_eq!(parse_opt_str(None), None);
        assert_eq!(parse_opt_str(Some(&Value::Null)), Some(None));
        assert_eq!(
            parse_opt_str(Some(&json!("wash-1"))),
            Some(Some("wash-1".into()))
        );
    }

    #[test]
    fn parse_entries_reads_the_node_on_pairs_the_editor_posts() {
        let set = json!([{ "node": "a", "on": true }, { "node": "b", "on": false }]);
        assert_eq!(
            parse_entries(&set).unwrap(),
            vec![SwitchEntry::node("a", true), SwitchEntry::node("b", false)]
        );
        assert!(
            parse_entries(&json!([])).unwrap().is_empty(),
            "an empty set is a valid new switch"
        );
        assert!(matches!(
            parse_entries(&json!([{ "node": "a" }])),
            Err(CallError::Bad(_))
        ));
        assert!(matches!(
            parse_entries(&json!({ "node": "a", "on": true })),
            Err(CallError::Bad(_))
        ));
    }

    /// A parameter entry carries `param`/`value` through untouched, and an
    /// entry that has neither parses exactly as it always did (`None`/`None`).
    #[test]
    fn parse_entries_carries_an_optional_param_and_value() {
        let set = json!([
            { "node": "a", "on": true },
            { "node": "b", "on": true, "param": "amount", "value": 0.5 }
        ]);
        let parsed = parse_entries(&set).unwrap();
        assert_eq!(parsed[0].param, None);
        assert_eq!(parsed[0].value, None);
        assert_eq!(parsed[1], SwitchEntry::param("b", "amount", 0.5));
        // `node`/`on` remain required even on a parameter entry.
        assert!(matches!(
            parse_entries(&json!([{ "param": "amount", "value": 0.5 }])),
            Err(CallError::Bad(_))
        ));
    }
}
