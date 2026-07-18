//! Agent loop events — the Pi `AgentEvent` contract consumed by frontends.
//!
//! Transient (emitted, not persisted). Modeled as a `type`-tagged enum whose
//! struct variants are flattened alongside the tag.

use serde_json::{Map, Value};

use crate::message::{AgentMessage, ToolResultMessage};
use crate::provider_event::AssistantMessageEvent;
use crate::tool_result::AgentToolResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStartEvent {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEndEvent {
    #[serde(default)]
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStartEvent {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnEndEvent {
    pub message: AgentMessage,
    #[serde(default)]
    pub tool_results: Vec<ToolResultMessage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageStartEvent {
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageUpdateEvent {
    pub message: AgentMessage,
    pub assistant_message_event: AssistantMessageEvent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageEndEvent {
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionStartEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub args: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionUpdateEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub args: Map<String, Value>,
    pub partial_result: AgentToolResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionEndEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub result: AgentToolResult,
    pub is_error: bool,
}

/// The full agent loop event union, discriminated by `type`.
///
/// `rename_all = "snake_case"` matches every Pi wire tag exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart(AgentStartEvent),
    AgentEnd(AgentEndEvent),
    TurnStart(TurnStartEvent),
    TurnEnd(TurnEndEvent),
    MessageStart(MessageStartEvent),
    MessageUpdate(Box<MessageUpdateEvent>),
    MessageEnd(MessageEndEvent),
    ToolExecutionStart(ToolExecutionStartEvent),
    ToolExecutionUpdate(ToolExecutionUpdateEvent),
    ToolExecutionEnd(ToolExecutionEndEvent),
}

impl AgentEvent {
    /// Wire tag string, matching Python event `.type`.
    pub fn tag(&self) -> &'static str {
        match self {
            AgentEvent::AgentStart(_) => "agent_start",
            AgentEvent::AgentEnd(_) => "agent_end",
            AgentEvent::TurnStart(_) => "turn_start",
            AgentEvent::TurnEnd(_) => "turn_end",
            AgentEvent::MessageStart(_) => "message_start",
            AgentEvent::MessageUpdate(_) => "message_update",
            AgentEvent::MessageEnd(_) => "message_end",
            AgentEvent::ToolExecutionStart(_) => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate(_) => "tool_execution_update",
            AgentEvent::ToolExecutionEnd(_) => "tool_execution_end",
        }
    }
}
