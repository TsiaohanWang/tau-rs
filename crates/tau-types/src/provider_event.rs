//! Canonical provider-neutral assistant stream events emitted by model
//! providers (the Pi `AssistantMessageEvent` contract).
//!
//! Shape: a `type`-tagged discriminated union. Each variant wraps an event
//! struct whose fields are flattened alongside the `type` tag. These events are
//! transient (not persisted); only deserialized by tests, so strict unknown-
//! field rejection is relaxed (see ADR-1 in `docs/phase-1.md`).

use crate::message::{AssistantMessage, SharedAssistant, ToolCall};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorReason {
    Aborted,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantStartEvent {
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextStartEvent {
    pub content_index: usize,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextDeltaEvent {
    pub content_index: usize,
    pub delta: String,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEndEvent {
    pub content_index: usize,
    pub content: String,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingStartEvent {
    pub content_index: usize,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingDeltaEvent {
    pub content_index: usize,
    pub delta: String,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThinkingEndEvent {
    pub content_index: usize,
    pub content: String,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallStartEvent {
    pub content_index: usize,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallDeltaEvent {
    pub content_index: usize,
    pub delta: String,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCallEndEvent {
    pub content_index: usize,
    pub tool_call: ToolCall,
    pub partial: SharedAssistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantDoneEvent {
    pub reason: DoneReason,
    pub message: AssistantMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantErrorEvent {
    pub reason: ErrorReason,
    pub error: AssistantMessage,
}

/// The full provider stream event union, discriminated by `type`.
///
/// `rename_all = "snake_case"` covers most variant tags; the `toolcall_*`
/// variants override with explicit renames because Pi's wire tag has no
/// underscore between "tool" and "call" (`toolcall_start`, not `tool_call_start`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start(AssistantStartEvent),
    TextStart(TextStartEvent),
    TextDelta(TextDeltaEvent),
    TextEnd(TextEndEvent),
    ThinkingStart(ThinkingStartEvent),
    ThinkingDelta(ThinkingDeltaEvent),
    ThinkingEnd(ThinkingEndEvent),
    #[serde(rename = "toolcall_start")]
    ToolCallStart(ToolCallStartEvent),
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta(ToolCallDeltaEvent),
    #[serde(rename = "toolcall_end")]
    ToolCallEnd(ToolCallEndEvent),
    Done(AssistantDoneEvent),
    Error(AssistantErrorEvent),
}

impl AssistantMessageEvent {
    /// Wire tag string, matching Python event `.type`.
    pub fn tag(&self) -> &'static str {
        match self {
            AssistantMessageEvent::Start(_) => "start",
            AssistantMessageEvent::TextStart(_) => "text_start",
            AssistantMessageEvent::TextDelta(_) => "text_delta",
            AssistantMessageEvent::TextEnd(_) => "text_end",
            AssistantMessageEvent::ThinkingStart(_) => "thinking_start",
            AssistantMessageEvent::ThinkingDelta(_) => "thinking_delta",
            AssistantMessageEvent::ThinkingEnd(_) => "thinking_end",
            AssistantMessageEvent::ToolCallStart(_) => "toolcall_start",
            AssistantMessageEvent::ToolCallDelta(_) => "toolcall_delta",
            AssistantMessageEvent::ToolCallEnd(_) => "toolcall_end",
            AssistantMessageEvent::Done(_) => "done",
            AssistantMessageEvent::Error(_) => "error",
        }
    }
}
