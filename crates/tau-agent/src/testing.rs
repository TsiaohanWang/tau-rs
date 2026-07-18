//! FakeProvider and event construction helpers ported from
//! `tests/pi_event_helpers.py` — test-only module gated behind `feature(testing)`.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use tau_types::{
    AssistantDoneEvent, AssistantErrorEvent, AssistantMessageEvent, AssistantStartEvent,
    DoneReason, ErrorReason, StopReason, TextDeltaEvent, ThinkingDeltaEvent, ToolCallEndEvent,
    Usage,
};

use crate::provider::{ModelProvider, StreamRequest};
// use crate::tool::AgentTool; // unused
use crate::tool::ProviderCall;

#[derive(Clone, Default)]
pub struct FakeProvider {
    batches: Arc<std::sync::Mutex<VecDeque<Vec<AssistantMessageEvent>>>>,
    pub called: Arc<AtomicBool>,
    pub calls: Arc<std::sync::Mutex<Vec<ProviderCall>>>,
}

impl FakeProvider {
    /// Create a provider that serves one batch of events per call.
    /// Mirrors Python's `FakeProvider([[event1, event2], [event3]])`.
    pub fn new(batches: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            batches: Arc::new(std::sync::Mutex::new(batches.into())),
            called: Arc::new(AtomicBool::new(false)),
            calls: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    /// Create a provider with a single batch (convenience).
    pub fn with_events(events: Vec<AssistantMessageEvent>) -> Self {
        Self::new(vec![events])
    }

    pub fn was_called(&self) -> bool {
        self.called.load(Ordering::Acquire)
    }

    pub fn recorded_calls(&self) -> Vec<ProviderCall> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

impl fmt::Debug for FakeProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeProvider").finish_non_exhaustive()
    }
}

#[async_trait]
impl ModelProvider for FakeProvider {
    fn stream_response<'a>(
        &'a self,
        request: &'a StreamRequest<'a>,
    ) -> BoxStream<'a, AssistantMessageEvent> {
        self.called.store(true, Ordering::Release);
        // Record the call for test assertions
        self.calls.lock().unwrap().push(ProviderCall {
            model: request.model.to_string(),
            system: request.system.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.to_vec(),
        });
        // Pop the next batch of events (empty stream if no more batches)
        let events = self.batches.lock().unwrap().pop_front().unwrap_or_default();
        stream::iter(events).boxed()
    }
}

// ---------------------------------------------------------------------------
// pi_events — ported from Python tests/pi_event_helpers.py
// ---------------------------------------------------------------------------

pub fn assistant_start(
    _stop_reason: Option<StopReason>,
    _usage: Option<Usage>,
) -> AssistantMessageEvent {
    AssistantMessageEvent::Start(AssistantStartEvent {
        partial: Arc::new(tau_types::AssistantMessage::default()),
    })
}

pub fn text_delta(text: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::TextDelta(TextDeltaEvent {
        content_index: 0,
        delta: text.to_string(),
        partial: Arc::new(tau_types::AssistantMessage::from_text(text)),
    })
}

pub fn thinking_delta(thinking: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::ThinkingDelta(ThinkingDeltaEvent {
        content_index: 0,
        delta: thinking.to_string(),
        partial: Arc::new({
            let mut m = tau_types::AssistantMessage::default();
            m.content.push(tau_types::AssistantContent::Thinking(
                tau_types::ThinkingContent::new(thinking),
            ));
            m
        }),
    })
}

pub fn tool_call_end(name: &str, arguments: &str, call_id: &str) -> AssistantMessageEvent {
    let mut tc = tau_types::ToolCall::new(call_id, name);
    if !arguments.is_empty() {
        if let Ok(map) =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(arguments)
        {
            tc.arguments = map;
        }
    }
    AssistantMessageEvent::ToolCallEnd(ToolCallEndEvent {
        content_index: 0,
        tool_call: tc.clone(),
        partial: Arc::new({
            let mut m = tau_types::AssistantMessage::default();
            m.content.push(tau_types::AssistantContent::ToolCall(tc));
            m
        }),
    })
}

pub fn assistant_done(message: tau_types::AssistantMessage) -> AssistantMessageEvent {
    let reason = if message.tool_calls().next().is_some() {
        DoneReason::ToolUse
    } else {
        match message.stop_reason {
            StopReason::Length => DoneReason::Length,
            StopReason::ToolUse => DoneReason::ToolUse,
            _ => DoneReason::Stop,
        }
    };
    AssistantMessageEvent::Done(AssistantDoneEvent { reason, message })
}

pub fn assistant_error(error: &str) -> AssistantMessageEvent {
    let msg = tau_types::AssistantMessage {
        stop_reason: StopReason::Error,
        error_message: Some(error.to_string()),
        ..Default::default()
    };
    AssistantMessageEvent::Error(AssistantErrorEvent {
        reason: ErrorReason::Error,
        error: msg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_provider_was_called_initially_false() {
        let p = FakeProvider::default();
        assert!(!p.was_called());
    }
}
