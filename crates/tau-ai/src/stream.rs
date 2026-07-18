//! Stream canonicalization: provider-specific events → Pi `AssistantMessageEvent`.
//!
//! Mirrors `tau_ai.stream.canonicalize_provider_stream`. A stateful async
//! generator that accumulates an `AssistantMessage` and emits `AssistantMessageEvent`
//! snapshots as provider events arrive.

use std::sync::Arc;

use async_stream::stream;
use futures::StreamExt;
use tau_types::{
    AssistantContent, AssistantDoneEvent, AssistantErrorEvent, AssistantMessage,
    AssistantMessageEvent, AssistantStartEvent, DoneReason, ErrorReason, StopReason, TextContent,
    TextDeltaEvent, TextEndEvent, TextStartEvent, ThinkingContent, ThinkingDeltaEvent,
    ThinkingEndEvent, ThinkingStartEvent, ToolCall, ToolCallEndEvent, ToolCallStartEvent,
};

/// Provider-neutral event emitted by model adapters.
///
/// Mirrors `tau_ai._provider_events.ProviderEvent`. Provider adapters
/// translate their raw SSE JSON into these before passing to the canonicalizer.
#[derive(Debug, Clone)]
pub enum ProviderEvent {
    ResponseStart {
        model: String,
    },
    TextDelta(String),
    ThinkingDelta(String),
    ToolCall(ToolCall),
    ResponseEnd {
        message: AssistantMessage,
        finish_reason: Option<String>,
    },
    Error {
        message: String,
        data: Option<serde_json::Value>,
    },
}

/// Canonicalize a stream of provider events into Pi `AssistantMessageEvent`s.
#[allow(unused_assignments)] // async_stream generator: assignments before yield are read
pub fn canonicalize_provider_stream(
    source: impl futures::Stream<Item = ProviderEvent> + Send + 'static,
) -> impl futures::Stream<Item = AssistantMessageEvent> + Send {
    stream! {
        let mut source = std::pin::pin!(source);
        let mut partial = AssistantMessage::default();
        let mut active_index: Option<usize> = None;
        let mut active_kind: Option<ActiveKind> = None;
        let mut started = false;

        while let Some(event) = source.next().await {
            match event {
                ProviderEvent::ResponseStart { model } => {
                    partial.model = model;
                    if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                }
                ProviderEvent::TextDelta(delta) => {
                    if active_kind != Some(ActiveKind::Text) {
                        // End any active block
                        if let (Some(kind), Some(idx)) = (active_kind, active_index) {
                            let content = match kind {
                                ActiveKind::Text => match partial.content.get(idx) {
                                    Some(AssistantContent::Text(tc)) => tc.text.clone(),
                                    _ => String::new(),
                                },
                                ActiveKind::Thinking => match partial.content.get(idx) {
                                    Some(AssistantContent::Thinking(tc)) => tc.thinking.clone(),
                                    _ => String::new(),
                                },
                            };
                            match kind {
                                ActiveKind::Text => {
                                    yield AssistantMessageEvent::TextEnd(TextEndEvent {
                                        content_index: idx,
                                        content,
                                        partial: Arc::new(partial.clone()),
                                    });
                                }
                                ActiveKind::Thinking => {
                                    yield AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
                                        content_index: idx,
                                        content,
                                        partial: Arc::new(partial.clone()),
                                    });
                                }
                            }
                        }
                        active_kind = Some(ActiveKind::Text);
                        let idx = partial.content.len();
                        partial.content.push(AssistantContent::Text(TextContent::new("")));
                        active_index = Some(idx);
                        if !started {
                            started = true;
                            yield AssistantMessageEvent::Start(AssistantStartEvent {
                                partial: Arc::new(partial.clone()),
                            });
                        }
                        yield AssistantMessageEvent::TextStart(TextStartEvent {
                            content_index: idx,
                            partial: Arc::new(partial.clone()),
                        });
                    } else if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                    let idx = active_index.unwrap_or(0);
                    if let Some(AssistantContent::Text(tc)) = partial.content.get_mut(idx) {
                        tc.text.push_str(&delta);
                    }
                    yield AssistantMessageEvent::TextDelta(TextDeltaEvent {
                        content_index: idx,
                        delta,
                        partial: Arc::new(partial.clone()),
                    });
                }
                ProviderEvent::ThinkingDelta(delta) => {
                    if active_kind != Some(ActiveKind::Thinking) {
                        if let (Some(kind), Some(idx)) = (active_kind, active_index) {
                            let content = match kind {
                                ActiveKind::Text => match partial.content.get(idx) {
                                    Some(AssistantContent::Text(tc)) => tc.text.clone(),
                                    _ => String::new(),
                                },
                                ActiveKind::Thinking => match partial.content.get(idx) {
                                    Some(AssistantContent::Thinking(tc)) => tc.thinking.clone(),
                                    _ => String::new(),
                                },
                            };
                            match kind {
                                ActiveKind::Text => {
                                    yield AssistantMessageEvent::TextEnd(TextEndEvent {
                                        content_index: idx,
                                        content,
                                        partial: Arc::new(partial.clone()),
                                    });
                                }
                                ActiveKind::Thinking => {
                                    yield AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
                                        content_index: idx,
                                        content,
                                        partial: Arc::new(partial.clone()),
                                    });
                                }
                            }
                        }
                        active_kind = Some(ActiveKind::Thinking);
                        let idx = partial.content.len();
                        partial.content.push(AssistantContent::Thinking(ThinkingContent::new("")));
                        active_index = Some(idx);
                        if !started {
                            started = true;
                            yield AssistantMessageEvent::Start(AssistantStartEvent {
                                partial: Arc::new(partial.clone()),
                            });
                        }
                        yield AssistantMessageEvent::ThinkingStart(ThinkingStartEvent {
                            content_index: idx,
                            partial: Arc::new(partial.clone()),
                        });
                    } else if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                    let idx = active_index.unwrap_or(0);
                    if let Some(AssistantContent::Thinking(tc)) = partial.content.get_mut(idx) {
                        tc.thinking.push_str(&delta);
                    }
                    yield AssistantMessageEvent::ThinkingDelta(ThinkingDeltaEvent {
                        content_index: idx,
                        delta,
                        partial: Arc::new(partial.clone()),
                    });
                }
                ProviderEvent::ToolCall(tc) => {
                    if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                    if let (Some(kind), Some(idx)) = (active_kind, active_index) {
                        let content = match kind {
                            ActiveKind::Text => match partial.content.get(idx) {
                                Some(AssistantContent::Text(tc)) => tc.text.clone(),
                                _ => String::new(),
                            },
                            ActiveKind::Thinking => match partial.content.get(idx) {
                                Some(AssistantContent::Thinking(tc)) => tc.thinking.clone(),
                                _ => String::new(),
                            },
                        };
                        match kind {
                            ActiveKind::Text => {
                                yield AssistantMessageEvent::TextEnd(TextEndEvent {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }
                            ActiveKind::Thinking => {
                                yield AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }
                        }
                    }
                    active_kind = None;
                    active_index = None;
                    let idx = partial.content.len();
                    partial.content.push(AssistantContent::ToolCall(tc.clone()));
                    yield AssistantMessageEvent::ToolCallStart(ToolCallStartEvent {
                        content_index: idx,
                        partial: Arc::new(partial.clone()),
                    });
                    yield AssistantMessageEvent::ToolCallEnd(ToolCallEndEvent {
                        content_index: idx,
                        tool_call: tc,
                        partial: Arc::new(partial.clone()),
                    });
                }
                ProviderEvent::ResponseEnd { mut message, finish_reason } => {
                    if let (Some(kind), Some(idx)) = (active_kind, active_index) {
                        let content = match kind {
                            ActiveKind::Text => match partial.content.get(idx) {
                                Some(AssistantContent::Text(tc)) => tc.text.clone(),
                                _ => String::new(),
                            },
                            ActiveKind::Thinking => match partial.content.get(idx) {
                                Some(AssistantContent::Thinking(tc)) => tc.thinking.clone(),
                                _ => String::new(),
                            },
                        };
                        match kind {
                            ActiveKind::Text => {
                                yield AssistantMessageEvent::TextEnd(TextEndEvent {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }
                            ActiveKind::Thinking => {
                                yield AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
                                    content_index: idx,
                                    content,
                                    partial: Arc::new(partial.clone()),
                                });
                            }
                        }
                    }
                    let has_tool_calls = partial.tool_calls().next().is_some();
                    let reason = map_finish_reason(&finish_reason, has_tool_calls);
                    let content = std::mem::take(&mut partial.content);
                    if !content.is_empty() {
                        message.content = content;
                    }
                    message.usage = partial.usage.clone();
                    if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                    yield AssistantMessageEvent::Done(AssistantDoneEvent {
                        reason,
                        message,
                    });
                    return;
                }
                ProviderEvent::Error { message: msg, .. } => {
                    if !started {
                        started = true;
                        yield AssistantMessageEvent::Start(AssistantStartEvent {
                            partial: Arc::new(partial.clone()),
                        });
                    }
                    let error = AssistantMessage {
                        stop_reason: StopReason::Error,
                        error_message: Some(msg),
                        ..Default::default()
                    };
                    yield AssistantMessageEvent::Error(AssistantErrorEvent {
                        reason: ErrorReason::Error,
                        error,
                    });
                    return;
                }
            }
        }
        // Stream ended without terminal event
        let error = AssistantMessage {
            stop_reason: StopReason::Error,
            error_message: Some("Provider stream ended without a terminal event".to_string()),
            ..Default::default()
        };
        yield AssistantMessageEvent::Error(AssistantErrorEvent {
            reason: ErrorReason::Error,
            error,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveKind {
    Text,
    Thinking,
}

/// Map provider finish_reason string to Pi's DoneReason.
fn map_finish_reason(reason: &Option<String>, has_tool_calls: bool) -> DoneReason {
    if has_tool_calls {
        return DoneReason::ToolUse;
    }
    match reason.as_deref() {
        Some("tool_calls" | "tool_use" | "toolUse") => DoneReason::ToolUse,
        Some("length" | "max_tokens" | "MAX_TOKENS" | "incomplete") => DoneReason::Length,
        _ => DoneReason::Stop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn canonicalize_simple_text() {
        let events = vec![
            ProviderEvent::ResponseStart {
                model: "test".into(),
            },
            ProviderEvent::TextDelta("Hello".into()),
            ProviderEvent::TextDelta(" world".into()),
            ProviderEvent::ResponseEnd {
                message: AssistantMessage::default(),
                finish_reason: None,
            },
        ];
        let mut out = std::pin::pin!(canonicalize_provider_stream(futures::stream::iter(events,)));

        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::Start(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextStart(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextDelta(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextDelta(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextEnd(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::Done(_)
        ));
        assert!(out.next().await.is_none());
    }

    #[tokio::test]
    async fn canonicalize_implicit_text_block() {
        let events = vec![
            ProviderEvent::TextDelta("Auto-started".into()),
            ProviderEvent::ResponseEnd {
                message: AssistantMessage::default(),
                finish_reason: None,
            },
        ];
        let mut out = std::pin::pin!(canonicalize_provider_stream(futures::stream::iter(events,)));

        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::Start(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextStart(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextDelta(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::TextEnd(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::Done(_)
        ));
    }

    #[tokio::test]
    async fn canonicalize_tool_use() {
        let tc = ToolCall::new("call-1".to_string(), "bash".to_string());
        let events = vec![
            ProviderEvent::ToolCall(tc),
            ProviderEvent::ResponseEnd {
                message: AssistantMessage::default(),
                finish_reason: Some("tool_use".into()),
            },
        ];
        let mut out = std::pin::pin!(canonicalize_provider_stream(futures::stream::iter(events,)));

        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::Start(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::ToolCallStart(_)
        ));
        assert!(matches!(
            out.next().await.unwrap(),
            AssistantMessageEvent::ToolCallEnd(_)
        ));
        let done = out.next().await.unwrap();
        if let AssistantMessageEvent::Done(d) = done {
            assert!(matches!(d.reason, DoneReason::ToolUse));
        } else {
            panic!("expected Done");
        }
    }
}
