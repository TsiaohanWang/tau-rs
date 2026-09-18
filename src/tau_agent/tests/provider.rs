//! Unit tests for [`crate::tau_agent::provider`].
//!
//! Test-only: compiled with `#[cfg(test)]` via `tau_agent::tests`.

use super::support::block_on;
use crate::tau_agent::messages::{AgentMessage, AssistantMessage};
use crate::tau_agent::provider::*;
use crate::tau_agent::provider_events::{
    AssistantMessageEvent, AssistantStartEvent, TextDeltaEvent,
};
use crate::tau_agent::tools::AgentTool;
use futures_core::Stream;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

struct FakeProvider;

struct VecStream(std::vec::IntoIter<AssistantMessageEvent>);

impl Stream for VecStream {
    type Item = AssistantMessageEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.get_mut().0.next())
    }
}

impl ModelProvider for FakeProvider {
    fn stream_response<'a>(
        &'a self,
        model: &'a str,
        _system: &'a str,
        messages: &'a [AgentMessage],
        tools: &'a [AgentTool],
        signal: Option<Arc<dyn CancellationToken>>,
        session_id: Option<&'a str>,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        assert_eq!(model, "fake");
        assert!(messages.is_empty());
        assert!(tools.is_empty());
        assert!(signal.is_none());
        assert_eq!(session_id, Some("session-1"));

        let partial = AssistantMessage {
            timestamp: 1,
            ..Default::default()
        };
        Box::pin(VecStream(
            vec![
                AssistantMessageEvent::AssistantStart(AssistantStartEvent {
                    partial: partial.clone(),
                }),
                AssistantMessageEvent::TextDelta(TextDeltaEvent {
                    content_index: 0,
                    delta: "d".into(),
                    partial,
                }),
            ]
            .into_iter(),
        ))
    }
}

#[test]
fn model_provider_streams_events_without_an_async_runtime() {
    let provider = FakeProvider;
    let mut stream = provider.stream_response("fake", "sys", &[], &[], None, Some("session-1"));

    assert!(matches!(
        block_on(poll_fn(|cx| stream.as_mut().poll_next(cx))),
        Some(AssistantMessageEvent::AssistantStart(_))
    ));
    assert!(matches!(
        block_on(poll_fn(|cx| stream.as_mut().poll_next(cx))),
        Some(AssistantMessageEvent::TextDelta(_))
    ));
    assert!(block_on(poll_fn(|cx| stream.as_mut().poll_next(cx))).is_none());
}
