//! Provider contract owned by the portable agent layer.
//!
//! Mirrors `tau_agent.provider.ModelProvider` / `CancellationToken`. The
//! `ModelProvider` trait returns a pull-based `BoxStream` of canonical
//! `AssistantMessageEvent`s, preserving Python's async-generator semantics
//! (drop = cancel) — see ADR-5 in `docs/phase-1.md`.

use futures::stream::BoxStream;
use tokio_util::sync::CancellationToken;

use tau_types::{AgentMessage, AssistantMessageEvent};

pub use tokio_util::sync::CancellationToken as CancelToken;

/// One streamed model response request, provider-neutral.
#[derive(Clone)]
pub struct StreamRequest<'a> {
    pub model: &'a str,
    pub system: &'a str,
    pub messages: &'a [AgentMessage],
    pub tools: &'a [AgentTool],
    pub signal: Option<CancellationToken>,
    /// Optional thinking/reasoning-effort level (e.g. `"low"`, `"high"`).
    /// Providers translate this into their vendor-specific parameter via the
    /// catalog `thinking_parameter` mapping. `None` leaves the provider
    /// default. Mirrors the original `tau_coding.thinking.ThinkingLevel`.
    pub thinking_level: Option<&'a str>,
}

use crate::tool::AgentTool;

/// Provider-neutral Pi-compatible model stream interface.
///
/// Implementations translate vendor SSE into `AssistantMessageEvent`s. The
/// returned stream is pull-based: the consumer drives it via
/// `StreamExt::next`, and dropping it cancels the response (equivalent to a
/// closed Python async generator).
pub trait ModelProvider: Send + Sync {
    fn stream_response<'a>(
        &'a self,
        request: &'a StreamRequest<'a>,
    ) -> BoxStream<'a, AssistantMessageEvent>;
}
