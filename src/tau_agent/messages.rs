//! Pi-compatible provider-neutral content and transcript message models.
//!
//! Rust port of `tau_agent/messages.py`. The JSON wire shape is identical:
//! camelCase keys, `role`/`type` discriminators, and `null` for absent
//! optional values.
//!
//! Two Python-only conveniences are intentionally gone:
//!
//! * the `WireModel` before-validators that accepted `content="..."` are
//!   replaced by [`assistant_content`] plus `..Default::default()`; Rust
//!   construction is statically typed, so the normalizer has no job.
//! * `WireModel`'s `validate_by_name` snake_case aliases are dropped; the
//!   session migration layer is the right place to rewrite legacy keys.

use serde::{Deserialize, Deserializer, Serialize};

use super::types::{JsonObject, JsonValue};

/// Return the current Unix timestamp in milliseconds.
pub(crate) fn current_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis() as u64
}

/// Default for provider/model fields the Python models default to `"unknown"`.
fn unknown() -> String {
    "unknown".to_owned()
}

/// Default for booleans the Python models default to `True`.
fn default_true() -> bool {
    true
}

/// Deserialize `null` as `T::default()`.
///
/// Mirrors `AssistantMessage._normalize_convenient_content`, which rewrites a
/// `None` usage to `Usage()`. `#[serde(default)]` alone only covers a missing
/// field, not an explicit `null` on the wire.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// The `role` discriminator shared by every transcript message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MessageRole {
    User,
    Assistant,
    ToolResult,
    BashExecution,
    Custom,
    BranchSummary,
    CompactionSummary,
}

/// Reject a role that does not belong to the message struct being deserialized.
fn expect_role<'de, D: Deserializer<'de>>(
    deserializer: D,
    expected: MessageRole,
) -> Result<MessageRole, D::Error> {
    let role = MessageRole::deserialize(deserializer)?;
    if role == expected {
        Ok(role)
    } else {
        Err(serde::de::Error::custom(format_args!(
            "expected {expected:?} message, got {role:?}"
        )))
    }
}

fn user_role<'de, D: Deserializer<'de>>(deserializer: D) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::User)
}

fn assistant_role<'de, D: Deserializer<'de>>(deserializer: D) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::Assistant)
}

fn tool_result_role<'de, D: Deserializer<'de>>(deserializer: D) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::ToolResult)
}

fn bash_execution_role<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::BashExecution)
}

fn custom_role<'de, D: Deserializer<'de>>(deserializer: D) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::Custom)
}

fn branch_summary_role<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::BranchSummary)
}

fn compaction_summary_role<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MessageRole, D::Error> {
    expect_role(deserializer, MessageRole::CompactionSummary)
}

/// Billed response cost in USD.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UsageCost {
    pub(crate) input: f64,
    pub(crate) output: f64,
    pub(crate) cache_read: f64,
    pub(crate) cache_write: f64,
    pub(crate) total: f64,
}

/// Provider-reported token usage for one assistant response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Usage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
    // Python's `_to_camel("cache_write_1h")` yields `cacheWrite1H` (str.title()
    // uppercases the `h`), while serde's camelCase yields `cacheWrite1h`.
    #[serde(rename = "cacheWrite1H")]
    pub(crate) cache_write_1h: Option<u64>,
    pub(crate) reasoning: Option<u64>,
    pub(crate) total_tokens: u64,
    pub(crate) cost: UsageCost,
}

/// Return the field-wise total for one or more provider requests.
pub(crate) fn sum_usage(usages: &[Usage]) -> Usage {
    fn optional_total(usages: &[Usage], get: impl Fn(&Usage) -> Option<u64>) -> Option<u64> {
        usages
            .iter()
            .filter_map(get)
            .reduce(|total, value| total + value)
    }

    Usage {
        input: usages.iter().map(|usage| usage.input).sum(),
        output: usages.iter().map(|usage| usage.output).sum(),
        cache_read: usages.iter().map(|usage| usage.cache_read).sum(),
        cache_write: usages.iter().map(|usage| usage.cache_write).sum(),
        cache_write_1h: optional_total(usages, |usage| usage.cache_write_1h),
        reasoning: optional_total(usages, |usage| usage.reasoning),
        total_tokens: usages.iter().map(|usage| usage.total_tokens).sum(),
        cost: UsageCost {
            input: usages.iter().map(|usage| usage.cost.input).sum(),
            output: usages.iter().map(|usage| usage.cost.output).sum(),
            cache_read: usages.iter().map(|usage| usage.cost.cache_read).sum(),
            cache_write: usages.iter().map(|usage| usage.cost.cache_write).sum(),
            total: usages.iter().map(|usage| usage.cost.total).sum(),
        },
    }
}

/// Monotonic request durations for one assistant response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ResponseTiming {
    pub(crate) time_to_first_output_ms: Option<u64>,
    pub(crate) total_duration_ms: u64,
}

/// A plain text content block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextContent {
    pub(crate) text: String,
    pub(crate) text_signature: Option<String>,
}

impl TextContent {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            text_signature: None,
        }
    }

    /// Visible text of this block.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// A reasoning content block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingContent {
    pub(crate) thinking: String,
    pub(crate) thinking_signature: Option<String>,
    #[serde(default)]
    pub(crate) redacted: bool,
}

/// A base64 image content block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ImageContent {
    pub(crate) data: String,
    pub(crate) mime_type: String,
}

/// A tool call content block requested by the assistant.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCall {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) arguments: JsonObject,
    pub(crate) thought_signature: Option<String>,
}

/// One block of a user message: text or image.
///
/// The Python model is an untagged `TextContent | ImageContent` union whose
/// `Literal` `type` field disambiguates it. In Rust the tag belongs to the
/// enum, so the payload structs do not carry a redundant `type` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum UserContentItem {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

impl UserContentItem {
    /// Visible text of this block; images contribute nothing.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Text(block) => block.text(),
            Self::Image(_) => "",
        }
    }
}

/// User message content: a bare string or a list of blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum UserContent {
    Str(String),
    List(Vec<UserContentItem>),
}

impl From<&str> for UserContent {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for UserContent {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<Vec<UserContentItem>> for UserContent {
    fn from(value: Vec<UserContentItem>) -> Self {
        Self::List(value)
    }
}

/// An ordered assistant content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AssistantContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "thinking")]
    Thinking(ThinkingContent),
    #[serde(rename = "toolCall")]
    ToolCall(ToolCall),
}

/// One block of a tool result: text or image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ToolResultContent {
    #[serde(rename = "text")]
    Text(TextContent),
    #[serde(rename = "image")]
    Image(ImageContent),
}

impl ToolResultContent {
    /// Visible text of this block; images contribute nothing.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Text(block) => block.text(),
            Self::Image(_) => "",
        }
    }
}

/// A user prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct UserMessage {
    #[serde(deserialize_with = "user_role")]
    pub(crate) role: MessageRole,
    pub(crate) content: UserContent,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

impl UserMessage {
    pub(crate) fn text(&self) -> String {
        content_text(&self.content)
    }
}

/// Structured provider error attached to an assistant diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AssistantDiagnosticErrorCode {
    Str(String),
    Int(i64),
}

/// Provider error details attached to an assistant diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantDiagnosticError {
    pub(crate) name: Option<String>,
    pub(crate) message: String,
    pub(crate) stack: Option<String>,
    pub(crate) code: Option<AssistantDiagnosticErrorCode>,
}

/// A non-fatal provider diagnostic recorded on an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantMessageDiagnostic {
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
    pub(crate) error: Option<AssistantDiagnosticError>,
    pub(crate) details: Option<JsonObject>,
}

/// Why the provider stopped generating an assistant response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum StopReason {
    #[default]
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// A Pi-compatible assistant message with ordered content blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantMessage {
    #[serde(deserialize_with = "assistant_role")]
    pub(crate) role: MessageRole,
    pub(crate) content: Vec<AssistantContent>,
    pub(crate) api: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) response_model: Option<String>,
    pub(crate) response_provider: Option<String>,
    pub(crate) response_id: Option<String>,
    pub(crate) diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(crate) usage: Usage,
    pub(crate) timing: Option<ResponseTiming>,
    pub(crate) stop_reason: StopReason,
    pub(crate) error_message: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

impl Default for AssistantMessage {
    fn default() -> Self {
        Self {
            role: MessageRole::Assistant,
            content: Vec::new(),
            api: unknown(),
            provider: unknown(),
            model: unknown(),
            response_model: None,
            response_provider: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            timing: None,
            stop_reason: StopReason::default(),
            error_message: None,
            timestamp: current_timestamp_ms(),
        }
    }
}

impl AssistantMessage {
    /// Concatenated visible text, ignoring thinking and tool call blocks.
    pub(crate) fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Text(text) => Some(text.text()),
                _ => None,
            })
            .collect()
    }

    /// Concatenated thinking text, ignoring visible and tool call blocks.
    pub(crate) fn thinking_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::Thinking(thinking) => Some(thinking.thinking.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Every tool call block, in content order.
    pub(crate) fn tool_calls(&self) -> Vec<&ToolCall> {
        self.content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::ToolCall(call) => Some(call),
                _ => None,
            })
            .collect()
    }
}

/// The result of executing one tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolResultMessage {
    #[serde(deserialize_with = "tool_result_role")]
    pub(crate) role: MessageRole,
    pub(crate) tool_call_id: String,
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) content: Vec<ToolResultContent>,
    pub(crate) details: Option<JsonValue>,
    pub(crate) added_tool_names: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) is_error: bool,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

impl ToolResultMessage {
    pub(crate) fn text(&self) -> String {
        self.content.iter().map(ToolResultContent::text).collect()
    }
}

/// A recorded user shell execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct BashExecutionMessage {
    #[serde(deserialize_with = "bash_execution_role")]
    pub(crate) role: MessageRole,
    pub(crate) command: String,
    pub(crate) output: String,
    pub(crate) exit_code: Option<i32>,
    #[serde(default)]
    pub(crate) cancelled: bool,
    #[serde(default)]
    pub(crate) truncated: bool,
    pub(crate) full_output_path: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
    #[serde(default)]
    pub(crate) exclude_from_context: bool,
}

/// An extension-injected message that participates in model context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CustomMessage {
    #[serde(deserialize_with = "custom_role")]
    pub(crate) role: MessageRole,
    pub(crate) custom_type: String,
    pub(crate) content: UserContent,
    #[serde(default = "default_true")]
    pub(crate) display: bool,
    pub(crate) details: Option<JsonValue>,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

impl CustomMessage {
    pub(crate) fn text(&self) -> String {
        content_text(&self.content)
    }
}

/// A summary of an abandoned branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct BranchSummaryMessage {
    #[serde(deserialize_with = "branch_summary_role")]
    pub(crate) role: MessageRole,
    pub(crate) summary: String,
    pub(crate) from_id: String,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

/// A summary that replaces older context after compaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CompactionSummaryMessage {
    #[serde(deserialize_with = "compaction_summary_role")]
    pub(crate) role: MessageRole,
    pub(crate) summary: String,
    pub(crate) tokens_before: u64,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

/// Any transcript message.
///
/// Python discriminates this union on the `role` key. Each variant validates
/// that key while deserializing, so `untagged` picks the right variant and a
/// serialized variant still carries its own `role` field (needed when an
/// assistant message is nested in a provider event).
// Variant names mirror the Python type names, and boxing the large assistant
// variant would only add indirection to a transcript that is not hot.
#[allow(clippy::enum_variant_names, clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AgentMessage {
    UserMessage(UserMessage),
    AssistantMessage(AssistantMessage),
    ToolResultMessage(ToolResultMessage),
    BashExecutionMessage(BashExecutionMessage),
    CustomMessage(CustomMessage),
    BranchSummaryMessage(BranchSummaryMessage),
    CompactionSummaryMessage(CompactionSummaryMessage),
}

impl From<UserMessage> for AgentMessage {
    fn from(message: UserMessage) -> Self {
        Self::UserMessage(message)
    }
}

impl From<AssistantMessage> for AgentMessage {
    fn from(message: AssistantMessage) -> Self {
        Self::AssistantMessage(message)
    }
}

impl From<ToolResultMessage> for AgentMessage {
    fn from(message: ToolResultMessage) -> Self {
        Self::ToolResultMessage(message)
    }
}

impl From<BashExecutionMessage> for AgentMessage {
    fn from(message: BashExecutionMessage) -> Self {
        Self::BashExecutionMessage(message)
    }
}

impl From<CustomMessage> for AgentMessage {
    fn from(message: CustomMessage) -> Self {
        Self::CustomMessage(message)
    }
}

impl From<BranchSummaryMessage> for AgentMessage {
    fn from(message: BranchSummaryMessage) -> Self {
        Self::BranchSummaryMessage(message)
    }
}

impl From<CompactionSummaryMessage> for AgentMessage {
    fn from(message: CompactionSummaryMessage) -> Self {
        Self::CompactionSummaryMessage(message)
    }
}

impl AgentMessage {
    /// The role of this message.
    pub(crate) fn role(&self) -> MessageRole {
        match self {
            Self::UserMessage(message) => message.role,
            Self::AssistantMessage(message) => message.role,
            Self::ToolResultMessage(message) => message.role,
            Self::BashExecutionMessage(message) => message.role,
            Self::CustomMessage(message) => message.role,
            Self::BranchSummaryMessage(message) => message.role,
            Self::CompactionSummaryMessage(message) => message.role,
        }
    }

    /// The creation timestamp of this message.
    pub(crate) fn timestamp(&self) -> u64 {
        match self {
            Self::UserMessage(message) => message.timestamp,
            Self::AssistantMessage(message) => message.timestamp,
            Self::ToolResultMessage(message) => message.timestamp,
            Self::BashExecutionMessage(message) => message.timestamp,
            Self::CustomMessage(message) => message.timestamp,
            Self::BranchSummaryMessage(message) => message.timestamp,
            Self::CompactionSummaryMessage(message) => message.timestamp,
        }
    }

    /// The user-visible text of this message.
    pub(crate) fn text(&self) -> String {
        message_text(self)
    }
}

/// Build canonical ordered assistant blocks from parser accumulators.
pub(crate) fn assistant_content(
    text: impl Into<String>,
    tool_calls: impl IntoIterator<Item = ToolCall>,
) -> Vec<AssistantContent> {
    let text = text.into();
    let mut blocks = Vec::new();
    if !text.is_empty() {
        blocks.push(AssistantContent::Text(TextContent::new(text)));
    }
    blocks.extend(tool_calls.into_iter().map(AssistantContent::ToolCall));
    blocks
}

/// Return visible text from string or text/image user content.
pub(crate) fn content_text(content: &UserContent) -> String {
    match content {
        UserContent::Str(text) => text.clone(),
        UserContent::List(items) => items.iter().map(UserContentItem::text).collect(),
    }
}

/// Convert custom/session-only messages to provider-compatible user context.
pub(crate) fn message_to_user(message: &AgentMessage) -> UserMessage {
    UserMessage {
        role: MessageRole::User,
        content: UserContent::Str(message_text(message)),
        timestamp: message.timestamp(),
    }
}

/// Return the user-visible text represented by an agent message.
///
/// The Python version needs `isinstance` checks and a fallthrough `return ""`.
/// The Rust enum is exhaustive, so every case is handled by the compiler.
pub(crate) fn message_text(message: &AgentMessage) -> String {
    match message {
        AgentMessage::UserMessage(message) => message.text(),
        AgentMessage::AssistantMessage(message) => message.text(),
        AgentMessage::ToolResultMessage(message) => message.text(),
        AgentMessage::BashExecutionMessage(message) => message.output.clone(),
        AgentMessage::CustomMessage(message) => message.text(),
        AgentMessage::BranchSummaryMessage(message) => message.summary.clone(),
        AgentMessage::CompactionSummaryMessage(message) => message.summary.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Expected strings below are produced by `tau_agent.messages` in Python.
    #[test]
    fn user_message_matches_python_wire_shape() {
        let message = UserMessage {
            role: MessageRole::User,
            content: "hello".into(),
            timestamp: 1,
        };

        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"user","content":"hello","timestamp":1}"#
        );
    }

    #[test]
    fn user_message_blocks_match_python_wire_shape() {
        let message = UserMessage {
            role: MessageRole::User,
            content: UserContent::List(vec![
                UserContentItem::Text(TextContent::new("a")),
                UserContentItem::Image(ImageContent {
                    data: "b64".into(),
                    mime_type: "image/png".into(),
                }),
            ]),
            timestamp: 1,
        };

        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"user","content":[{"type":"text","text":"a","textSignature":null},{"type":"image","data":"b64","mimeType":"image/png"}],"timestamp":1}"#
        );
    }

    #[test]
    fn assistant_message_matches_python_wire_shape() {
        let message = AssistantMessage {
            role: MessageRole::Assistant,
            content: vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: "t".into(),
                    thinking_signature: None,
                    redacted: true,
                }),
                AssistantContent::Text(TextContent {
                    text: "a".into(),
                    text_signature: Some("sig".into()),
                }),
                AssistantContent::ToolCall(ToolCall {
                    id: "c1".into(),
                    name: "read".into(),
                    arguments: JsonObject::from_iter([("path".to_owned(), json!("x"))]),
                    thought_signature: Some("ts".into()),
                }),
            ],
            api: "api".into(),
            provider: "p".into(),
            model: "m".into(),
            response_model: Some("rm".into()),
            response_provider: Some("rp".into()),
            response_id: Some("rid".into()),
            diagnostics: Some(vec![AssistantMessageDiagnostic {
                r#type: "warn".into(),
                timestamp: 1,
                error: Some(AssistantDiagnosticError {
                    name: Some("E".into()),
                    message: "m".into(),
                    stack: Some("s".into()),
                    code: Some(AssistantDiagnosticErrorCode::Int(7)),
                }),
                details: Some(JsonObject::from_iter([("k".to_owned(), json!(1))])),
            }]),
            usage: Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
                cache_write_1h: Some(5),
                reasoning: Some(6),
                total_tokens: 7,
                cost: UsageCost::default(),
            },
            timing: Some(ResponseTiming {
                time_to_first_output_ms: Some(8),
                total_duration_ms: 9,
            }),
            stop_reason: StopReason::ToolUse,
            error_message: Some("err".into()),
            timestamp: 1,
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            concat!(
                r#"{"role":"assistant","content":[{"type":"thinking","thinking":"t","thinkingSignature":null,"redacted":true},"#,
                r#"{"type":"text","text":"a","textSignature":"sig"},"#,
                r#"{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"x"},"thoughtSignature":"ts"}],"#,
                r#""api":"api","provider":"p","model":"m","responseModel":"rm","responseProvider":"rp","responseId":"rid","#,
                r#""diagnostics":[{"type":"warn","timestamp":1,"error":{"name":"E","message":"m","stack":"s","code":7},"details":{"k":1}}],"#,
                r#""usage":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4,"cacheWrite1H":5,"reasoning":6,"totalTokens":7,"#,
                r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"#,
                r#""timing":{"timeToFirstOutputMs":8,"totalDurationMs":9},"stopReason":"toolUse","errorMessage":"err","timestamp":1}"#
            )
        );
    }

    #[test]
    fn assistant_minimal_matches_python_wire_shape() {
        let message = AssistantMessage {
            timestamp: 1,
            ..Default::default()
        };

        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            concat!(
                r#"{"role":"assistant","content":[],"api":"unknown","provider":"unknown","model":"unknown","#,
                r#""responseModel":null,"responseProvider":null,"responseId":null,"diagnostics":null,"#,
                r#""usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"cacheWrite1H":null,"reasoning":null,"totalTokens":0,"#,
                r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"#,
                r#""timing":null,"stopReason":"stop","errorMessage":null,"timestamp":1}"#
            )
        );
    }

    #[test]
    fn tool_result_message_matches_python_wire_shape() {
        let message = ToolResultMessage {
            role: MessageRole::ToolResult,
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: vec![ToolResultContent::Text(TextContent::new("out"))],
            details: Some(json!({"a": 1})),
            added_tool_names: Some(vec!["x".into()]),
            is_error: true,
            timestamp: 1,
        };

        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"out","textSignature":null}],"details":{"a":1},"addedToolNames":["x"],"isError":true,"timestamp":1}"#
        );
    }

    #[test]
    fn session_only_messages_match_python_wire_shape() {
        let bash = BashExecutionMessage {
            role: MessageRole::BashExecution,
            command: "ls".into(),
            output: "o".into(),
            exit_code: Some(0),
            cancelled: true,
            truncated: true,
            full_output_path: Some("/tmp/x".into()),
            timestamp: 1,
            exclude_from_context: true,
        };
        let custom = CustomMessage {
            role: MessageRole::Custom,
            custom_type: "ct".into(),
            content: UserContent::List(vec![UserContentItem::Text(TextContent::new("x"))]),
            display: false,
            details: None,
            timestamp: 1,
        };
        let branch = BranchSummaryMessage {
            role: MessageRole::BranchSummary,
            summary: "s".into(),
            from_id: "f".into(),
            timestamp: 1,
        };
        let compaction = CompactionSummaryMessage {
            role: MessageRole::CompactionSummary,
            summary: "s".into(),
            tokens_before: 3,
            timestamp: 1,
        };

        assert_eq!(
            serde_json::to_string(&bash).unwrap(),
            r#"{"role":"bashExecution","command":"ls","output":"o","exitCode":0,"cancelled":true,"truncated":true,"fullOutputPath":"/tmp/x","timestamp":1,"excludeFromContext":true}"#
        );
        assert_eq!(
            serde_json::to_string(&custom).unwrap(),
            r#"{"role":"custom","customType":"ct","content":[{"type":"text","text":"x","textSignature":null}],"display":false,"details":null,"timestamp":1}"#
        );
        assert_eq!(
            serde_json::to_string(&branch).unwrap(),
            r#"{"role":"branchSummary","summary":"s","fromId":"f","timestamp":1}"#
        );
        assert_eq!(
            serde_json::to_string(&compaction).unwrap(),
            r#"{"role":"compactionSummary","summary":"s","tokensBefore":3,"timestamp":1}"#
        );
    }

    #[test]
    fn agent_message_union_dispatches_on_role() {
        let cases = [
            (
                r#"{"role":"user","content":"hello","timestamp":1}"#,
                MessageRole::User,
            ),
            (
                r#"{"role":"assistant","content":[],"api":"unknown","provider":"unknown","model":"unknown","timestamp":1}"#,
                MessageRole::Assistant,
            ),
            (
                r#"{"role":"toolResult","toolCallId":"c1","toolName":"read","timestamp":1}"#,
                MessageRole::ToolResult,
            ),
            (
                r#"{"role":"bashExecution","command":"ls","output":"o","timestamp":1}"#,
                MessageRole::BashExecution,
            ),
            (
                r#"{"role":"custom","customType":"ct","content":"x","timestamp":1}"#,
                MessageRole::Custom,
            ),
            (
                r#"{"role":"branchSummary","summary":"s","fromId":"f","timestamp":1}"#,
                MessageRole::BranchSummary,
            ),
            (
                r#"{"role":"compactionSummary","summary":"s","tokensBefore":3,"timestamp":1}"#,
                MessageRole::CompactionSummary,
            ),
        ];

        for (payload, role) in cases {
            let message: AgentMessage = serde_json::from_str(payload).unwrap();
            assert_eq!(message.role(), role);
            let round_tripped: AgentMessage =
                serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
            assert_eq!(round_tripped, message);
        }
    }

    #[test]
    fn agent_message_validates_roles_and_unknown_fields() {
        // A minimal assistant payload must not fall into the user variant.
        let message: AgentMessage =
            serde_json::from_str(r#"{"role":"assistant","content":[{"type":"text","text":"x"}]}"#)
                .unwrap();
        assert!(matches!(message, AgentMessage::AssistantMessage(_)));

        // A minimal user payload must not fall into the assistant variant.
        let message: AgentMessage =
            serde_json::from_str(r#"{"role":"user","content":[{"type":"text","text":"x"}]}"#)
                .unwrap();
        assert!(matches!(message, AgentMessage::UserMessage(_)));

        assert!(serde_json::from_str::<AgentMessage>(r#"{"role":"nope","content":"x"}"#).is_err());
        assert!(
            serde_json::from_str::<AgentMessage>(
                r#"{"role":"user","content":"x","unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<UserMessage>(r#"{"role":"assistant","content":"x"}"#).is_err()
        );
    }

    #[test]
    fn content_items_validate_the_type_tag() {
        assert!(serde_json::from_str::<UserContentItem>(r#"{"type":"image","text":"x"}"#).is_err());
        assert!(
            serde_json::from_str::<UserContentItem>(
                r#"{"type":"text","data":"x","mimeType":"image/png"}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<UserContentItem>(r#"{"type":"other"}"#).is_err());
    }

    #[test]
    fn assistant_deserialization_uses_python_defaults() {
        let message: AssistantMessage =
            serde_json::from_str(r#"{"role":"assistant","timestamp":1}"#).unwrap();
        assert_eq!(message.api, "unknown");
        assert_eq!(message.provider, "unknown");
        assert_eq!(message.model, "unknown");
        assert!(message.content.is_empty());
        assert_eq!(message.usage, Usage::default());
        assert_eq!(message.stop_reason, StopReason::Stop);

        // Python's before-validator turns an explicit null usage into Usage().
        let message: AssistantMessage =
            serde_json::from_str(r#"{"role":"assistant","usage":null,"timestamp":1}"#).unwrap();
        assert_eq!(message.usage, Usage::default());

        // stop reason uses the camelCase wire spelling.
        let message: AssistantMessage =
            serde_json::from_str(r#"{"role":"assistant","stopReason":"toolUse"}"#).unwrap();
        assert_eq!(message.stop_reason, StopReason::ToolUse);
    }

    #[test]
    fn sum_usage_matches_python_optional_semantics() {
        let usages = [
            Usage {
                input: 1,
                cache_write_1h: Some(2),
                ..Usage::default()
            },
            Usage {
                output: 3,
                reasoning: Some(4),
                ..Usage::default()
            },
        ];
        let total = sum_usage(&usages);

        assert_eq!(total.input, 1);
        assert_eq!(total.output, 3);
        assert_eq!(total.cache_write_1h, Some(2));
        assert_eq!(total.reasoning, Some(4));

        assert_eq!(sum_usage(&[]).cache_write_1h, None);
        assert_eq!(sum_usage(&[Usage::default()]).cache_write_1h, None);
    }

    #[test]
    fn assistant_accessors_mirror_python_properties() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        };
        let message = AssistantMessage {
            content: assistant_content("answer", [call.clone()]),
            timestamp: 1,
            ..Default::default()
        };

        assert_eq!(message.text(), "answer");
        assert_eq!(message.thinking_text(), "");
        assert_eq!(message.tool_calls(), vec![&call]);

        let message = AssistantMessage {
            content: vec![
                AssistantContent::Thinking(ThinkingContent {
                    thinking: "why".into(),
                    thinking_signature: None,
                    redacted: false,
                }),
                AssistantContent::Text(TextContent::new("answer")),
            ],
            timestamp: 1,
            ..Default::default()
        };
        assert_eq!(message.thinking_text(), "why");
        assert_eq!(message.text(), "answer");
    }

    #[test]
    fn content_text_skips_images_and_message_text_dispatches() {
        let mixed: Vec<ToolResultContent> = vec![
            ToolResultContent::Text(TextContent::new("a")),
            ToolResultContent::Image(ImageContent {
                data: "b64".into(),
                mime_type: "image/png".into(),
            }),
        ];
        let message = ToolResultMessage {
            role: MessageRole::ToolResult,
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: mixed,
            details: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 1,
        };
        assert_eq!(message.text(), "a");

        assert_eq!(
            content_text(&UserContent::List(vec![
                UserContentItem::Text(TextContent::new("a")),
                UserContentItem::Image(ImageContent {
                    data: "b64".into(),
                    mime_type: "image/png".into(),
                }),
            ])),
            "a"
        );

        let bash = BashExecutionMessage {
            role: MessageRole::BashExecution,
            command: "ls".into(),
            output: "out".into(),
            exit_code: None,
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 7,
            exclude_from_context: false,
        };
        let message = AgentMessage::BashExecutionMessage(bash);
        assert_eq!(message_text(&message), "out");
        assert_eq!(message.text(), "out");
        assert_eq!(message.timestamp(), 7);
        assert_eq!(message.role(), MessageRole::BashExecution);
    }

    #[test]
    fn message_to_user_preserves_text_and_timestamp() {
        let custom = CustomMessage {
            role: MessageRole::Custom,
            custom_type: "note".into(),
            content: "remember".into(),
            display: true,
            details: None,
            timestamp: 42,
        };
        let user = message_to_user(&custom.into());

        assert_eq!(user.role, MessageRole::User);
        assert_eq!(user.timestamp, 42);
        assert_eq!(user.text(), "remember");
        assert_eq!(
            serde_json::to_string(&user).unwrap(),
            r#"{"role":"user","content":"remember","timestamp":42}"#
        );
    }

    #[test]
    fn assistant_content_builds_ordered_blocks() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        };

        assert_eq!(
            assistant_content("hello", [call.clone()]),
            vec![
                AssistantContent::Text(TextContent::new("hello")),
                AssistantContent::ToolCall(call),
            ]
        );
        assert!(assistant_content("", Vec::<ToolCall>::new()).is_empty());
    }
}
