//! Public re-exports of the canonical assistant stream event contract.

// The Python facade carries `# ruff: noqa: F401`; nothing in the crate consumes
// these re-exports yet, they exist for the provider adapters.
#![allow(unused_imports)]

pub(crate) use crate::tau_agent::provider_events::{
    AssistantDoneEvent, AssistantErrorEvent, AssistantMessageEvent, AssistantStartEvent,
    DoneReason, ErrorReason, TextDeltaEvent, TextEndEvent, TextStartEvent, ThinkingDeltaEvent,
    ThinkingEndEvent, ThinkingStartEvent, ToolCallDeltaEvent, ToolCallEndEvent, ToolCallStartEvent,
};
