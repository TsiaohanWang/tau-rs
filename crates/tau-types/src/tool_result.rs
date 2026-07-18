//! Tool execution result data (the portable agent's `AgentToolResult`).
//!
//! Mirrors `tau_agent.tools.AgentToolResult`. Pure data; the executor trait
//! lives in `tau-agent`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::{TextContent, ToolResultContent};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentToolResult {
    #[serde(default)]
    pub content: Vec<ToolResultContent>,
    /// `JSONValue` in Python; null is omitted on serialize (matches
    /// `exclude_none=True`).
    #[serde(default = "Value::default", skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

impl AgentToolResult {
    /// Empty result, mirroring `AgentToolResult(content=[], details={})`.
    pub fn empty() -> Self {
        AgentToolResult {
            content: Vec::new(),
            details: Value::Object(serde_json::Map::new()),
            added_tool_names: None,
            terminate: None,
        }
    }

    /// Convenience constructor mirroring Python's string-content acceptance.
    pub fn from_text(text: impl Into<String>) -> Self {
        let t = text.into();
        let mut r = AgentToolResult::default();
        if !t.is_empty() {
            r.content.push(ToolResultContent::Text(TextContent::new(t)));
        }
        r
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ToolResultContent::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    }
}
