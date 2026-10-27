//! `tools/list`, computed from metal-agent's catalog (rule 7: nothing hand-listed).

use metal_agent::harness::TOOL_CATALOG;
use metal_agent::tool_schemas;
use serde_json::{json, Value};

/// Names under this prefix are retired and must never be exported (plan
/// section 4). The catalog carries none today; the filter stays so one cannot
/// leak back in by being added to the catalog.
pub fn is_retired(name: &str) -> bool {
    name.starts_with("ai.")
}

/// Whether `name` is a tool this server offers: implemented, not retired, and
/// in the catalog.
pub fn is_exported(name: &str) -> bool {
    !is_retired(name) && TOOL_CATALOG.iter().any(|(n, _, ok)| *ok && *n == name)
}

/// The MCP `Tool` objects for every exported tool, in catalog order.
pub fn tools() -> Vec<Value> {
    TOOL_CATALOG
        .iter()
        .filter(|(name, _, ok)| *ok && !is_retired(name))
        .filter_map(|(name, desc, _)| {
            let meta = tool_schemas::meta(name)?;
            let schema = tool_schemas::parameters(name)?;
            Some(json!({
                "name": name,
                "description": desc,
                "inputSchema": schema,
                "annotations": {
                    "readOnlyHint": meta.readonly,
                    "destructiveHint": meta.destructive,
                },
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every exported tool's inputSchema is a well-formed object schema, equals
    /// what `tool_schemas` computes for it, survives a JSON text round trip,
    /// and its annotations agree with the catalog's meta. The expected count
    /// is computed from `TOOL_CATALOG`, not typed in.
    #[test]
    fn every_exported_schema_is_valid_and_matches_tool_schemas() {
        let tools = tools();
        let want = TOOL_CATALOG
            .iter()
            .filter(|(n, _, ok)| *ok && !is_retired(n))
            .count();
        assert!(want > 0);
        assert_eq!(tools.len(), want);

        const JSON_TYPES: &[&str] = &["string", "number", "integer", "boolean", "array", "object"];
        for tool in &tools {
            let name = tool["name"].as_str().expect("name");
            let schema = &tool["inputSchema"];
            assert_eq!(schema["type"], "object", "{name}: schema type");
            let props = schema["properties"].as_object().expect("properties object");
            for req in schema["required"].as_array().expect("required array") {
                let key = req.as_str().expect("required entries are strings");
                assert!(props.contains_key(key), "{name}: required `{key}` not in properties");
            }
            for (key, prop) in props {
                let ty = prop["type"].as_str().unwrap_or_else(|| panic!("{name}.{key}: no type"));
                assert!(JSON_TYPES.contains(&ty), "{name}.{key}: unknown type {ty}");
            }

            assert_eq!(
                schema,
                &tool_schemas::parameters(name).expect("catalog name"),
                "{name}: schema differs from tool_schemas"
            );
            let text = schema.to_string();
            let back: Value = serde_json::from_str(&text).expect("schema re-parses");
            assert_eq!(&back, schema, "{name}: schema changed on round trip");

            let meta = tool_schemas::meta(name).expect("catalog meta");
            assert_eq!(tool["annotations"]["readOnlyHint"], meta.readonly, "{name}");
            assert_eq!(tool["annotations"]["destructiveHint"], meta.destructive, "{name}");
        }
    }

    #[test]
    fn retired_and_unimplemented_tools_are_not_exported() {
        assert!(is_retired("ai.gen"));
        assert!(!is_exported("ai.gen"));
        assert!(is_exported("capture_output"), "E2: the node capture route and executor land it");
        assert!(!is_exported("no_such_tool"));
        assert!(is_exported("list_nodes"));
    }
}
