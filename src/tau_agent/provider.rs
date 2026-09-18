//! Provider contract owned by Tau's portable agent layer.
//!
//! Rust port of `tau_agent/provider.py`. The deliberate deviations:
//!
//! * **`AsyncIterator[AssistantMessageEvent]` becomes a boxed `futures_core::Stream`.**
//!   `stream_response` returns
//!   `Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>`, the same
//!   shape as `futures_util::stream::BoxStream`. Tokio integration stays natural:
//!   `tokio_stream` adapts this type and `StreamExt::next` drives it (ADR-010).
//! * **Keyword-only arguments become positional references.** `model`,
//!   `system`, `messages`, `tools`, and `session_id` are borrowed for the
//!   lifetime of the returned stream; providers clone what they need to keep.
//! * **`signal` is `Option<Arc<dyn CancellationToken>>`** so it can be cloned
//!   into whatever task a provider spawns.
//! * The Python `Protocol` becomes a Rust trait with `Send + Sync` bounds,
//!   which the stream and the future loop need to move providers across threads.

use std::pin::Pin;
use std::sync::Arc;

use futures_core::Stream;

use super::messages::AgentMessage;
use super::provider_events::AssistantMessageEvent;
use super::tools::AgentTool;

/// Return whether the current stream should stop.
pub(crate) trait CancellationToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// Provider-neutral Pi-compatible model stream interface.
pub(crate) trait ModelProvider: Send + Sync {
    /// Stream one model response as assistant message events.
    ///
    /// Providers may use `session_id` for request routing or prompt-cache
    /// affinity. Unsupported providers ignore it.
    fn stream_response<'a>(
        &'a self,
        model: &'a str,
        system: &'a str,
        messages: &'a [AgentMessage],
        tools: &'a [AgentTool],
        signal: Option<Arc<dyn CancellationToken>>,
        session_id: Option<&'a str>,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>>;
}
