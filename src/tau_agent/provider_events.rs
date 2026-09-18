//! Pi-compatible assistant stream events owned by the portable agent layer.
//!
//! Rust port of `tau_agent/provider_events.py`. The wire JSON is identical;
//! this header collects the places where the Rust design differs and why.
//!
//! ## Deliberate deviations
//!
//! * **The tag lives on the enum, not on each class.** Python repeats
//!   `type: Literal["..."] = "..."` on every event class so pydantic can
//!   discriminate the union. Rust has no pydantic, so
//!   [`AssistantMessageEvent`] is `#[serde(tag = "type")]` and the payload
//!   structs carry no `type` field. Serializing a variant writes the same
//!   `{"type": "...", ...}` object as Python, and each variant rename spells
//!   the Python tag (note `toolcall_start`, without the underscore).
//! * **No standalone payload deserialization.** Because the payload structs do
//!   not carry the tag, only the union can be deserialized. Python can also
//!   `model_validate` a single event class (its `type` has a default), but the
//!   port never needs that: providers construct events, and the loop matches on
//!   the union. This avoids a second, tag-carrying representation of every
//!   event just to serve validation.
//! * **`content_index` is `usize`** instead of Python's unbounded `int`. It is
//!   used to index `partial.content`, and the type rejects negative indices.
//! * **`DoneReason`/`ErrorReason` are separate enums**, mirroring the two
//!   Python `Literal` aliases, so a `done` event cannot report `aborted` and an
//!   `error` event cannot report `stop`.
//! * **The `partial()` accessor returns `Option`**, because Python's `.partial`
//!   attribute does not exist on `done`/`error` (those carry `message`/`error`).
//!
//! The snake_case aliases on multi-word fields exist because Python's
//! `WireModel` sets `validate_by_name=True`; the wire JSON always uses the
//! camelCase spelling.

use serde::{Deserialize, Serialize};

use super::messages::{AssistantMessage, ToolCall};

/// The first event of an assistant stream, carrying the initial message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantStartEvent {
    pub(crate) partial: AssistantMessage,
}

/// A text content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextStartEvent {
    // Python's unbounded `int`; `usize` matches the field's use as an index into
    // `partial.content` and rejects negative values. The alias accepts the
    // snake_case name because `WireModel` sets `validate_by_name=True`; the
    // remaining event fields follow the same pattern.
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of text was appended to a text content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A text content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) content: String,
    pub(crate) partial: AssistantMessage,
}

/// A thinking content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingStartEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of reasoning was appended to a thinking content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A thinking content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) content: String,
    pub(crate) partial: AssistantMessage,
}

/// A tool call content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallStartEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of tool call arguments was appended to a tool call block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A tool call content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    #[serde(alias = "tool_call")]
    pub(crate) tool_call: ToolCall,
    pub(crate) partial: AssistantMessage,
}

/// Why the provider stopped generating a response.
///
/// Python's `DoneReason = Literal["stop", "length", "toolUse"]`; the enum
/// keeps the same values and keeps `aborted` out of the success path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

/// Why the provider stream failed.
///
/// Python's `ErrorReason = Literal["aborted", "error"]`; the reverse of
/// [`DoneReason`], so `stop` cannot appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ErrorReason {
    Aborted,
    Error,
}

/// The assistant stream finished successfully.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantDoneEvent {
    pub(crate) reason: DoneReason,
    pub(crate) message: AssistantMessage,
}

/// The assistant stream failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantErrorEvent {
    pub(crate) reason: ErrorReason,
    pub(crate) error: AssistantMessage,
}

/// Any assistant stream event.
///
/// `#[serde(tag = "type")]` reproduces Python's discriminated union: the
/// payload structs above omit `type`, the enum writes it, and only the enum can
/// be deserialized. See the module header for why the port accepts that
/// single-entry-point trade-off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AssistantMessageEvent {
    #[serde(rename = "start")]
    AssistantStart(AssistantStartEvent),
    #[serde(rename = "text_start")]
    TextStart(TextStartEvent),
    #[serde(rename = "text_delta")]
    TextDelta(TextDeltaEvent),
    #[serde(rename = "text_end")]
    TextEnd(TextEndEvent),
    #[serde(rename = "thinking_start")]
    ThinkingStart(ThinkingStartEvent),
    #[serde(rename = "thinking_delta")]
    ThinkingDelta(ThinkingDeltaEvent),
    #[serde(rename = "thinking_end")]
    ThinkingEnd(ThinkingEndEvent),
    // Python spells these tags without an underscore between the words.
    #[serde(rename = "toolcall_start")]
    ToolCallStart(ToolCallStartEvent),
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta(ToolCallDeltaEvent),
    #[serde(rename = "toolcall_end")]
    ToolCallEnd(ToolCallEndEvent),
    #[serde(rename = "done")]
    Done(AssistantDoneEvent),
    #[serde(rename = "error")]
    Error(AssistantErrorEvent),
}

impl AssistantMessageEvent {
    /// The partial assistant message carried by stream update events.
    ///
    /// Replaces Python's `event.partial` attribute access: `done` and `error`
    /// carry a final message instead, so the Rust accessor is an `Option`.
    pub(crate) fn partial(&self) -> Option<&AssistantMessage> {
        match self {
            Self::AssistantStart(event) => Some(&event.partial),
            Self::TextStart(event) => Some(&event.partial),
            Self::TextDelta(event) => Some(&event.partial),
            Self::TextEnd(event) => Some(&event.partial),
            Self::ThinkingStart(event) => Some(&event.partial),
            Self::ThinkingDelta(event) => Some(&event.partial),
            Self::ThinkingEnd(event) => Some(&event.partial),
            Self::ToolCallStart(event) => Some(&event.partial),
            Self::ToolCallDelta(event) => Some(&event.partial),
            Self::ToolCallEnd(event) => Some(&event.partial),
            Self::Done(_) | Self::Error(_) => None,
        }
    }
}
