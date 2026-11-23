//! Provider adapters: one per wire format. The runner and the message type
//! stay provider-neutral; only [`Provider::send`] knows whether the bytes on
//! the wire are OpenAI chat-completions or Anthropic `/v1/messages`.
//!
//! From Continue's `openai-adapters` split and ZeroClaw's `anthropic.rs` /
//! `compatible.rs`. [`for_config`] picks the adapter from the preset, which is
//! the fix for problem 2 (the Claude preset used to send an OpenAI body).

use crate::config::{Preset, ProviderConfig};

use super::message::ChatMessage;
use super::retry::ProviderError;

pub mod anthropic;
pub mod openai;

/// One streamed fragment: assistant text, or a tool-call argument chunk.
#[derive(Debug, Clone, Default)]
pub struct Delta {
    pub text: String,
    /// Set once the fragment belongs to a tool call.
    pub call_id: Option<String>,
    pub name: Option<String>,
    pub args_delta: Option<String>,
}

/// Everything a provider adapter needs for one request.
pub struct Request<'a> {
    pub system: Option<&'a str>,
    pub messages: &'a [ChatMessage],
    pub tools: &'a [serde_json::Value],
}

/// A provider that turns a conversation into one assistant step (text and/or
/// tool calls). Streaming, if any, is delivered through `on_delta` before the
/// final [`ChatMessage`] is returned; `on_delta` is called synchronously.
pub trait Provider: Send {
    fn send(
        &self,
        req: &Request,
        on_delta: &mut dyn FnMut(Delta),
    ) -> Result<ChatMessage, ProviderError>;

    /// A short id for logs (`openai` | `anthropic`).
    fn wire(&self) -> &'static str;
}

/// The adapter for a saved config: Anthropic's own wire for [`Preset::Claude`],
/// the OpenAI-compatible wire for everything else (OpenAI, DeepInfra, Manual).
pub fn for_config(cfg: &ProviderConfig) -> Box<dyn Provider> {
    match cfg.preset {
        Preset::Claude => Box::new(anthropic::Anthropic::from_config(cfg)),
        _ => Box::new(openai::OpenAiCompat::from_config(cfg)),
    }
}
