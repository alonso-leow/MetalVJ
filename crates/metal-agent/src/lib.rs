//! `metal-agent`: the provider/config/loop/MCP-client scaffold for an AI
//! harness agent (H.3 in `the design notes`), plus H.4's tools.
//!
//! - [`config`]: provider config — `[agent.provider]` in `config.toml` for
//!   preset/url/model (S.AH), `data/ai.json` for the key (Claude/OpenAI/
//!   DeepInfra presets, or manual) — with a hand-written masked `Debug` so
//!   the API key never lands in a log line.
//! - [`images`]: turn-scoped in-memory image persistence.
//! - [`mcp`]: a synchronous MCP client over stdio and HTTP transports.
//! - [`tools`]: the tool registry, with native-tools-win-collisions baked
//!   into how MCP tools get inserted.
//! - [`agent_loop`]: the prompt -> message+tool-calls -> results -> repeat
//!   loop skeleton and the [`agent_loop::ToolExecutor`] trait.
//! - [`harness`]: H.4 — [`harness::Harness`], the real `ToolExecutor` that
//!   implements twelve of the seventeen tools as thin wrappers over
//!   `metal-graph`/`metal-forge`. See that module's doc comment for exactly which
//!   five are out of scope and why.
//! - [`http`]: the hand-rolled blocking HTTP client both `agent_loop` and
//!   `mcp`'s HTTP transport share. No async runtime, matching the rest of
//!   the workspace.
//! - [`voice`]: H.6 — sync STT (audio -> text) and TTS (text -> audio) calls,
//!   layered over the H.3 loop rather than wired into it. Configured by the
//!   optional `stt`/`tts` entries on [`config::ProviderConfig`]; absent means
//!   "no voice", not an error.
//! - [`shots`]: H.7 — loads and validates `data/agent/shots.json` (the
//!   few-shot corpus) and `data/agent/system_prompt.md` (the alma) against
//!   the real tool surface in [`harness::TOOL_CATALOG`].
//! - [`turn`]: O.2 — the agent's *turn* as a level of its own, between the
//!   single tool call and the loop. Holds what outlives a call (the images,
//!   the working directory), chains I.3's four gates over it, and hosts the
//!   two-pass `locate` protocol, which needs the model to choose between
//!   calls and so cannot live in [`harness::Harness`].

pub mod agent_loop;
pub mod auth;
pub mod chat;
pub mod config;
pub mod harness;
pub mod http;
pub mod images;
pub mod locate;
pub mod mcp;
pub mod session;
pub mod shots;
pub mod tool_schemas;
pub mod tools;
pub mod venue_gen;
pub mod turn;
pub mod voice;
