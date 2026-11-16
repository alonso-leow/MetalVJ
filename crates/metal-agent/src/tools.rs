//! Tool registration. H.3's rule: "Native tools always win on a name collision."
//! Native tools don't exist yet (that's H.4) — this is
//! just the structure that makes the rule mechanical once they do:
//! native tools are inserted first, and MCP tools are inserted with
//! `entry().or_insert()` so a name already claimed by a native tool is never
//! overwritten.

use serde_json::Value;
use std::collections::HashMap;

/// Where a registered tool came from — kept alongside the entry so a caller
/// (e.g. `list_tools`, H.4's first tool) can report provenance rather than
/// just a flat name list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    Native,
    Mcp { server: String },
}

pub struct RegisteredTool {
    pub source: ToolSource,
    /// The raw tool spec (schema, description) as the provider's
    /// tool-calling dialect expects it. Native tools build this themselves
    /// (H.4); MCP tools pass through [`crate::mcp::McpTool::spec`].
    pub spec: Value,
}

/// Insert-once-native-first: call [`register_native`](Self::register_native)
/// for every native tool before calling
/// [`register_mcp`](Self::register_mcp) for any MCP server's tools, and
/// collisions resolve to the native tool automatically.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a native tool. Native tools always win, so this always
    /// overwrites whatever was there — the ordering contract (native
    /// registered first) is what a caller must uphold, not something this
    /// method can enforce on its own without knowing the full tool set
    /// upfront.
    pub fn register_native(&mut self, name: impl Into<String>, spec: Value) {
        self.tools.insert(name.into(), RegisteredTool { source: ToolSource::Native, spec });
    }

    /// Registers an MCP-provided tool under its prefixed name
    /// (`{server}.{tool}`). If a native tool already claimed that exact
    /// name, the MCP one is silently dropped — native wins.
    pub fn register_mcp(&mut self, server: &str, prefixed_name: impl Into<String>, spec: Value) {
        self.tools.entry(prefixed_name.into()).or_insert_with(|| RegisteredTool {
            source: ToolSource::Mcp { server: server.to_string() },
            spec,
        });
    }

    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Every registered spec, in a stable order, as the `tools` array a
    /// request carries.
    ///
    /// Sorted by name because a `HashMap` is not ordered and an unordered
    /// tool list makes two identical turns produce two different request
    /// bodies — which defeats provider-side prompt caching and makes a
    /// recorded request impossible to diff against a replayed one.
    pub fn specs(&self) -> Vec<Value> {
        let mut names: Vec<&str> = self.names().collect();
        names.sort_unstable();
        names.iter().filter_map(|n| self.tools.get(*n)).map(|t| t.spec.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_tool_wins_a_name_collision_with_mcp() {
        let mut reg = ToolRegistry::new();
        reg.register_native("get_env", json!({"native": true}));
        // An MCP server happens to also expose a tool literally named
        // "get_env" — it must not shadow the native one.
        reg.register_mcp("weird_server", "get_env", json!({"native": false}));

        let entry = reg.get("get_env").unwrap();
        assert_eq!(entry.source, ToolSource::Native);
        assert_eq!(entry.spec["native"], true);
    }

    #[test]
    fn mcp_tools_without_collision_register_fine() {
        let mut reg = ToolRegistry::new();
        reg.register_native("get_env", json!({}));
        reg.register_mcp("blender", "blender.render", json!({"schema": "x"}));

        assert_eq!(reg.len(), 2);
        assert_eq!(reg.get("blender.render").unwrap().source, ToolSource::Mcp { server: "blender".into() });
    }
}
