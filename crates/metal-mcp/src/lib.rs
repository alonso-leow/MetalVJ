//! MCP adapter for metal-agent's tool catalog (plan section 4).
//!
//! Three layers, none of which knows a tool's behaviour:
//! - [`catalog`]: `tools/list` is computed from `TOOL_CATALOG` and
//!   `tool_schemas`; retired `ai.*` names are never exported.
//! - [`dash`]: `tools/call` forwards the arguments to the route metal-dash's own
//!   `GET /ai-config` names for that tool (its `route` field), 1:1.
//! - [`rpc`] and [`transport`]: JSON-RPC 2.0 framing over stdio or
//!   Streamable HTTP (`POST /mcp`).

pub mod catalog;
pub mod dash;
pub mod rpc;
pub mod transport;

/// The MCP protocol revision this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
