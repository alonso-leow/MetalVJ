//! `metal-agent`'s chat component: the one place messages, provider calls,
//! retries and function calling live. The harness (`TOOL_CATALOG` +
//! `Harness::execute`) is untouched — it stays the tool backend; this module
//! is what the *model* runs through.
//!
//! Split after ZeroClaw's `zeroclaw-providers` / `zeroclaw-runtime::agent`
//! and Continue's client session state, at our scale:
//!
//! - [`message`]: the provider-neutral [`message::ChatMessage`] and the
//!   per-call [`message::ToolCallStatus`] lifecycle.
//! - [`events`]: the [`events::Event`] the runner emits and the store folds.
//! - [`provider`]: the [`provider::Provider`] trait and the two adapters.
//! - [`retry`]: [`retry::ProviderError`] classification and `with_retry`.
//! - [`policy`]: per-tool [`policy::ToolPolicy`] (`auto`/`ask`/`off`).
//! - [`history`]: pruning by whole exchanges.
//! - [`runner`]: the loop that ties them together (`run_session_turn`).

pub mod events;
pub mod history;
pub mod message;
pub mod policy;
pub mod provider;
pub mod retry;
pub mod runner;

pub use events::Event;
pub use message::{ChatMessage, ToolCallState, ToolCallStatus};
pub use policy::ToolPolicy;
pub use provider::{Request, Provider};
pub use retry::{ErrorKind, ProviderError, RetryPolicy};
pub use runner::{Approval, run_session_turn};
