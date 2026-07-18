//! Provider-neutral Pi-compatible wire models: content blocks, transcript
//! messages, usage. Serialized shape matches Tau's Python `WireModel` exactly
//! (camelCase aliases, `role`-tagged discriminated union, `exclude_none`).
//!
//! Strictness: unknown fields are rejected on the top-level `AgentMessage`
//! union via a hand-written `Deserialize` (see ADR-1 in `docs/phase-1.md`),
//! because serde's internally-tagged enum cannot honor `deny_unknown_fields`.
//! Nested content blocks keep derived (lenient-on-inner-fields) deserialize
//! but still reject unknown `type` tags.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

// ---------------------------------------------------------------------------
// timestamps & ids
// ---------------------------------------------------------------------------

/// Unix epoch milliseconds, matching Python's `current_timestamp_ms`.
pub fn current_timestamp_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Fresh entry id matching Python's `uuid4().hex` (32 lowercase hex chars).
pub fn new_entry_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// ---------------------------------------------------------------------------
// discriminator tag enums
// ---------------------------------------------------------------------------

/// The `role` discriminator of an `AgentMessage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Assistant,
    ToolResult,
    BashExecution,
    Custom,
    BranchSummary,
    CompactionSummary,
}

/// The `type` discriminator of an assistant content block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentBlockType {
    Text,
    Thinking,
    Image,
    ToolCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub enum StopReason {
    #[default]
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

// ---------------------------------------------------------------------------
// usage
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageCost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    #[serde(default)]
    pub total: f64,
}

impl Default for UsageCost {
    fn default() -> Self {
        UsageCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[derive(Default)]
pub struct Usage {
    #[serde(default)]
    pub input: i64,
    #[serde(default)]
    pub output: i64,
    #[serde(default)]
    pub cache_read: i64,
    #[serde(default)]
    pub cache_write: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<i64>,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub cost: UsageCost,
}

// ---------------------------------------------------------------------------
// assistant diagnostics
// ---------------------------------------------------------------------------

/// `code: str | int | None` in Python — untagged union of string/int (None is
/// represented by `Option::None` and skipped on serialize).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DiagnosticCode {
    Str(String),
    Int(i64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantDiagnosticError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<DiagnosticCode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<AssistantDiagnosticError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Map<String, Value>>,
}

// ---------------------------------------------------------------------------
// content blocks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextContent {
    #[serde(rename = "type", default = "block_text")]
    pub r#type: ContentBlockType,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

impl TextContent {
    pub fn new(text: impl Into<String>) -> Self {
        TextContent {
            r#type: ContentBlockType::Text,
            text: text.into(),
            text_signature: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThinkingContent {
    #[serde(rename = "type", default = "block_thinking")]
    pub r#type: ContentBlockType,
    pub thinking: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default)]
    pub redacted: bool,
}

impl ThinkingContent {
    pub fn new(thinking: impl Into<String>) -> Self {
        ThinkingContent {
            r#type: ContentBlockType::Thinking,
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageContent {
    #[serde(rename = "type", default = "block_image")]
    pub r#type: ContentBlockType,
    pub data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCall {
    #[serde(rename = "type", default = "block_tool_call")]
    pub r#type: ContentBlockType,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ToolCall {
            r#type: ContentBlockType::ToolCall,
            id: id.into(),
            name: name.into(),
            arguments: Map::new(),
            thought_signature: None,
        }
    }
}

/// Assistant content block — `type`-tagged discriminated union.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssistantContent {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
}

impl From<TextContent> for AssistantContent {
    fn from(b: TextContent) -> Self {
        AssistantContent::Text(b)
    }
}

impl From<ThinkingContent> for AssistantContent {
    fn from(b: ThinkingContent) -> Self {
        AssistantContent::Thinking(b)
    }
}

impl From<ToolCall> for AssistantContent {
    fn from(b: ToolCall) -> Self {
        AssistantContent::ToolCall(b)
    }
}

/// User-visible content block (text or image).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserBlock {
    Text(TextContent),
    Image(ImageContent),
}

impl From<TextContent> for UserBlock {
    fn from(b: TextContent) -> Self {
        UserBlock::Text(b)
    }
}

impl From<ImageContent> for UserBlock {
    fn from(b: ImageContent) -> Self {
        UserBlock::Image(b)
    }
}

/// Either a plain text string or a list of text/image blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<UserBlock>),
}

impl UserContent {
    pub fn text(s: impl Into<String>) -> Self {
        UserContent::Text(s.into())
    }
}

impl From<String> for UserContent {
    fn from(s: String) -> Self {
        UserContent::Text(s)
    }
}

impl From<&str> for UserContent {
    fn from(s: &str) -> Self {
        UserContent::Text(s.to_string())
    }
}

/// Tool-result content block — `type`-tagged (text or image).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(TextContent),
    Image(ImageContent),
}

impl From<TextContent> for ToolResultContent {
    fn from(b: TextContent) -> Self {
        ToolResultContent::Text(b)
    }
}

impl From<ImageContent> for ToolResultContent {
    fn from(b: ImageContent) -> Self {
        ToolResultContent::Image(b)
    }
}

// ---------------------------------------------------------------------------
// transcript messages
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserMessage {
    #[serde(default = "role_user")]
    pub role: MessageRole,
    pub content: UserContent,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl UserMessage {
    pub fn new(content: impl Into<UserContent>) -> Self {
        UserMessage {
            role: MessageRole::User,
            content: content.into(),
            timestamp: current_timestamp_ms(),
        }
    }
    pub fn text(&self) -> String {
        content_text(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssistantMessage {
    #[serde(default = "role_assistant")]
    pub role: MessageRole,
    #[serde(default)]
    pub content: Vec<AssistantContent>,
    #[serde(default = "default_unknown")]
    pub api: String,
    #[serde(default = "default_unknown")]
    pub provider: String,
    #[serde(default = "default_unknown")]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub stop_reason: StopReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl Default for AssistantMessage {
    fn default() -> Self {
        AssistantMessage {
            role: MessageRole::Assistant,
            content: Vec::new(),
            api: "unknown".into(),
            provider: "unknown".into(),
            model: "unknown".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: current_timestamp_ms(),
        }
    }
}

impl AssistantMessage {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut m = AssistantMessage::default();
        if !text.is_empty() {
            m.content
                .push(AssistantContent::Text(TextContent::new(text)));
        }
        m
    }

    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                AssistantContent::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn thinking_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                AssistantContent::Thinking(t) => Some(t.thinking.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|b| match b {
            AssistantContent::ToolCall(c) => Some(c),
            _ => None,
        })
    }
}

// Build a flat assistant message from ordered content parts. Mirrors Python's
// `assistant_content(text, tool_calls)`.
pub fn assistant_content_parts(
    text: &str,
    tool_calls: impl IntoIterator<Item = ToolCall>,
) -> Vec<AssistantContent> {
    let mut blocks: Vec<AssistantContent> = Vec::new();
    if !text.is_empty() {
        blocks.push(AssistantContent::Text(TextContent::new(text)));
    }
    for tc in tool_calls {
        blocks.push(AssistantContent::ToolCall(tc));
    }
    blocks
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResultMessage {
    #[serde(default = "role_tool_result")]
    pub role: MessageRole,
    pub tool_call_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub content: Vec<ToolResultContent>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl ToolResultMessage {
    pub fn new(tool_call_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        ToolResultMessage {
            role: MessageRole::ToolResult,
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: Vec::new(),
            details: Value::Null,
            added_tool_names: None,
            is_error: false,
            timestamp: current_timestamp_ms(),
        }
    }
    pub fn text(&self) -> String {
        toolresult_content_text(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BashExecutionMessage {
    #[serde(default = "role_bash_execution")]
    pub role: MessageRole,
    pub command: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub cancelled: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
    #[serde(default)]
    pub exclude_from_context: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomMessage {
    #[serde(default = "role_custom")]
    pub role: MessageRole,
    pub custom_type: String,
    pub content: UserContent,
    #[serde(default = "default_true")]
    pub display: bool,
    #[serde(default = "Value::default", skip_serializing_if = "Value::is_null")]
    pub details: Value,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

impl CustomMessage {
    pub fn text(&self) -> String {
        content_text(&self.content)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchSummaryMessage {
    #[serde(default = "role_branch_summary")]
    pub role: MessageRole,
    pub summary: String,
    pub from_id: String,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactionSummaryMessage {
    #[serde(default = "role_compaction_summary")]
    pub role: MessageRole,
    pub summary: String,
    pub tokens_before: i64,
    #[serde(default = "current_timestamp_ms")]
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// AgentMessage union (role-tagged)
// ---------------------------------------------------------------------------

/// Provider-neutral transcript message union, discriminated by `role`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AgentMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
    BashExecution(BashExecutionMessage),
    Custom(CustomMessage),
    BranchSummary(BranchSummaryMessage),
    CompactionSummary(CompactionSummaryMessage),
}

impl AgentMessage {
    pub fn role(&self) -> MessageRole {
        match self {
            AgentMessage::User(_) => MessageRole::User,
            AgentMessage::Assistant(_) => MessageRole::Assistant,
            AgentMessage::ToolResult(_) => MessageRole::ToolResult,
            AgentMessage::BashExecution(_) => MessageRole::BashExecution,
            AgentMessage::Custom(_) => MessageRole::Custom,
            AgentMessage::BranchSummary(_) => MessageRole::BranchSummary,
            AgentMessage::CompactionSummary(_) => MessageRole::CompactionSummary,
        }
    }

    pub fn timestamp(&self) -> i64 {
        match self {
            AgentMessage::User(m) => m.timestamp,
            AgentMessage::Assistant(m) => m.timestamp,
            AgentMessage::ToolResult(m) => m.timestamp,
            AgentMessage::BashExecution(m) => m.timestamp,
            AgentMessage::Custom(m) => m.timestamp,
            AgentMessage::BranchSummary(m) => m.timestamp,
            AgentMessage::CompactionSummary(m) => m.timestamp,
        }
    }

    pub fn text(&self) -> String {
        match self {
            AgentMessage::User(m) => m.text(),
            AgentMessage::Assistant(m) => m.text(),
            AgentMessage::ToolResult(m) => m.text(),
            AgentMessage::Custom(m) => m.text(),
            AgentMessage::BranchSummary(m) => m.summary.clone(),
            AgentMessage::CompactionSummary(m) => m.summary.clone(),
            AgentMessage::BashExecution(m) => m.output.clone(),
        }
    }

    pub fn tool_call_id(&self) -> Option<&str> {
        match self {
            AgentMessage::ToolResult(m) => Some(&m.tool_call_id),
            _ => None,
        }
    }
}

/// Convert custom/session-only messages to provider-compatible user context.
/// Mirrors Python's `message_to_user`.
pub fn message_to_user(message: &AgentMessage) -> UserMessage {
    UserMessage {
        role: MessageRole::User,
        content: UserContent::Text(message.text()),
        timestamp: message.timestamp(),
    }
}

/// Visible text from string or text/image content. Mirrors `content_text`.
pub fn content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(s) => s.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|b| match b {
                UserBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect(),
    }
}

fn toolresult_content_text(content: &[ToolResultContent]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ToolResultContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Manual Deserialize for discriminated unions (strict, see ADR-1)
// ---------------------------------------------------------------------------

impl<'de> Deserialize<'de> for AgentMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| de::Error::custom("agent message must be a JSON object"))?;
        let role = obj
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("agent message missing `role` field"))?;
        match role {
            "user" => serde_json::from_value::<UserMessage>(value)
                .map(AgentMessage::User)
                .map_err(de::Error::custom),
            "assistant" => serde_json::from_value::<AssistantMessage>(value)
                .map(AgentMessage::Assistant)
                .map_err(de::Error::custom),
            "toolResult" => serde_json::from_value::<ToolResultMessage>(value)
                .map(AgentMessage::ToolResult)
                .map_err(de::Error::custom),
            "bashExecution" => serde_json::from_value::<BashExecutionMessage>(value)
                .map(AgentMessage::BashExecution)
                .map_err(de::Error::custom),
            "custom" => serde_json::from_value::<CustomMessage>(value)
                .map(AgentMessage::Custom)
                .map_err(de::Error::custom),
            "branchSummary" => serde_json::from_value::<BranchSummaryMessage>(value)
                .map(AgentMessage::BranchSummary)
                .map_err(de::Error::custom),
            "compactionSummary" => serde_json::from_value::<CompactionSummaryMessage>(value)
                .map(AgentMessage::CompactionSummary)
                .map_err(de::Error::custom),
            other => Err(de::Error::custom(format!(
                "unknown agent message role: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// default value helpers
// ---------------------------------------------------------------------------

fn role_user() -> MessageRole {
    MessageRole::User
}
fn role_assistant() -> MessageRole {
    MessageRole::Assistant
}
fn role_tool_result() -> MessageRole {
    MessageRole::ToolResult
}
fn role_bash_execution() -> MessageRole {
    MessageRole::BashExecution
}
fn role_custom() -> MessageRole {
    MessageRole::Custom
}
fn role_branch_summary() -> MessageRole {
    MessageRole::BranchSummary
}
fn role_compaction_summary() -> MessageRole {
    MessageRole::CompactionSummary
}
fn block_text() -> ContentBlockType {
    ContentBlockType::Text
}
fn block_thinking() -> ContentBlockType {
    ContentBlockType::Thinking
}
fn block_image() -> ContentBlockType {
    ContentBlockType::Image
}
fn block_tool_call() -> ContentBlockType {
    ContentBlockType::ToolCall
}
fn default_unknown() -> String {
    "unknown".to_string()
}
fn default_true() -> bool {
    true
}

/// A shared `AssistantMessage` snapshot carried by stream events (see ADR-2).
pub type SharedAssistant = Arc<AssistantMessage>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_roundtrip() {
        let m = UserMessage::new("hi");
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"role\":\"user\""));
        assert!(json.contains("\"content\":\"hi\""));
        assert!(json.contains("\"timestamp\""));
        let back: UserMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(m.content, back.content);
    }

    #[test]
    fn agent_message_dispatches_by_role() {
        let json = r#"{"role":"assistant","content":[{"type":"text","text":"hi"}],"model":"m"}"#;
        let m: AgentMessage = serde_json::from_str(json).unwrap();
        match m {
            AgentMessage::Assistant(a) => assert_eq!(a.text(), "hi"),
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn agent_message_rejects_unknown_role() {
        let json = r#"{"role":"wat","content":"x"}"#;
        let err = serde_json::from_str::<AgentMessage>(json).unwrap_err();
        assert!(err.to_string().contains("unknown agent message role"));
    }

    #[test]
    fn agent_message_strict_unknown_field_on_variant() {
        // AssistantMessage has deny_unknown_fields; an unexpected key is rejected.
        let json = r#"{"role":"assistant","content":[],"bogus":1}"#;
        assert!(serde_json::from_str::<AgentMessage>(json).is_err());
    }
}
