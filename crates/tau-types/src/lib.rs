#![deny(unsafe_code)]
//! Tau wire contract: provider-neutral Pi data models shared by every layer.
//!
//! This crate is intentionally free of async runtime and HTTP concerns. It owns
//! the serde models (`message`, `event`, `provider_event`, `session`,
//! `tool_result`) that cross the provider/agent/application boundaries.
//!
//! Serialization matches Tau's Python `WireModel` protocol byte-for-byte
//! (camelCase aliases, `role`/`type` discriminated unions, `exclude_none`),
//! so Rust and Python can read and write the same `~/.tau` artifacts.

pub mod event;
pub mod message;
pub mod provider_event;
pub mod session;
pub mod tool_result;

pub use event::{
    AgentEndEvent, AgentEvent, AgentStartEvent, MessageEndEvent, MessageStartEvent,
    MessageUpdateEvent, ToolExecutionEndEvent, ToolExecutionStartEvent, ToolExecutionUpdateEvent,
    TurnEndEvent, TurnStartEvent,
};
pub use message::{
    AgentMessage, AssistantContent, AssistantDiagnosticError, AssistantMessage,
    AssistantMessageDiagnostic, BashExecutionMessage, BranchSummaryMessage,
    CompactionSummaryMessage, ContentBlockType, CustomMessage, DiagnosticCode, ImageContent,
    MessageRole, StopReason, TextContent, ThinkingContent, ToolCall, ToolResultContent,
    ToolResultMessage, Usage, UserBlock, UserContent, UserMessage, assistant_content_parts,
    content_text, current_timestamp_ms, message_to_user, new_entry_id,
};
pub use provider_event::{
    AssistantDoneEvent, AssistantErrorEvent, AssistantMessageEvent, AssistantStartEvent,
    DoneReason, ErrorReason, TextDeltaEvent, TextEndEvent, TextStartEvent, ThinkingDeltaEvent,
    ThinkingEndEvent, ThinkingStartEvent, ToolCallDeltaEvent, ToolCallEndEvent, ToolCallStartEvent,
};
pub use session::{
    BranchSummaryEntry, CompactionEntry, CustomEntry, EntryType, LabelEntry, LeafEntry,
    MessageEntry, ModelChangeEntry, SessionEntry, SessionInfoEntry, ThinkingLevelChangeEntry,
    current_timestamp_secs,
};
pub use tool_result::AgentToolResult;
