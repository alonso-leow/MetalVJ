//! Per-tool policy: `auto` (run it), `ask` (pause for the operator), `off`
//! (never offer it to the model). From Continue's `ToolPolicy` and ZeroClaw's
//! `approval`: the destructive tools default to `ask`, the rest to `auto`,
//! and the default comes from the `readonly`/`destructive` flags in
//! [`crate::tool_schemas`] so there is one place that decides.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    Auto,
    Ask,
    Off,
}

impl ToolPolicy {
    pub fn wire(self) -> &'static str {
        match self {
            ToolPolicy::Auto => "auto",
            ToolPolicy::Ask => "ask",
            ToolPolicy::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<ToolPolicy> {
        Some(match s {
            "auto" | "on" | "true" => ToolPolicy::Auto,
            "ask" => ToolPolicy::Ask,
            "off" | "false" => ToolPolicy::Off,
            _ => return None,
        })
    }

    /// The default for a tool the operator has not set: destructive → `ask`,
    /// everything else → `auto`. Unknown tools fall to `ask` (fail safe).
    pub fn default_for(name: &str) -> ToolPolicy {
        match crate::tool_schemas::meta(name) {
            Some(meta) if meta.destructive => ToolPolicy::Ask,
            Some(_) => ToolPolicy::Auto,
            None => ToolPolicy::Ask,
        }
    }
}

/// The policy table, read from `data/ai_tools.json`'s `policy` object with
/// legacy booleans folded in (D10/D11): `true → auto`, `false → off`, absent
/// → [`ToolPolicy::default_for`].
pub fn resolve(name: &str, saved: &Value) -> ToolPolicy {
    let table = saved.get("policy").or_else(|| saved.get("tools"));
    if let Some(map) = table.and_then(Value::as_object) {
        if let Some(v) = map.get(name) {
            if let Some(s) = v.as_str().and_then(ToolPolicy::parse) {
                return s;
            }
            if let Some(b) = v.as_bool() {
                return if b { ToolPolicy::Auto } else { ToolPolicy::Off };
            }
        }
    }
    ToolPolicy::default_for(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_is_ask_for_destructive_and_auto_otherwise() {
        // `delete_node` is flagged destructive in `tool_schemas`.
        assert_eq!(ToolPolicy::default_for("delete_node"), ToolPolicy::Ask);
        assert_eq!(ToolPolicy::default_for("list_nodes"), ToolPolicy::Auto);
        assert_eq!(ToolPolicy::default_for("no_such_tool"), ToolPolicy::Ask);
    }

    #[test]
    fn legacy_booleans_fold_into_auto_off() {
        let saved = json!({ "tools": { "delete_node": true, "list_nodes": false } });
        assert_eq!(resolve("delete_node", &saved), ToolPolicy::Auto);
        assert_eq!(resolve("list_nodes", &saved), ToolPolicy::Off);
        assert_eq!(resolve("get_env", &saved), ToolPolicy::Auto);
    }

    #[test]
    fn the_new_policy_table_wins() {
        let saved = json!({ "policy": { "delete_node": "off" } });
        assert_eq!(resolve("delete_node", &saved), ToolPolicy::Off);
    }
}
