//! Pi-compatible provider-neutral content and transcript message models.
//!
//! Rust port of `tau_agent/messages.py`. The wire JSON is identical; this
//! header collects every place where the Rust design deliberately differs from
//! the Python one, because those choices are the reason the file is not a
//! literal transliteration.
//!
//! ## Input contract (kept, with Rust mechanics)
//!
//! * camelCase keys serialize because Python's `WireModel` sets
//!   `serialize_by_alias=True`. Its `validate_by_name=True` also accepts the
//!   snake_case field names, so every multi-word field repeats the Python name
//!   as `#[serde(alias = "...")]`; the legacy session loader depends on this
//!   (old JSONL stores `usage.cache_read`).
//! * `#[serde(deny_unknown_fields)]` mirrors `extra="forbid"`.
//! * absent options serialize as `null`, like pydantic's default dump.
//!   `exclude_none=True`, used by the JSONL session writer, is a per-call
//!   serialization concern and stays out of the model layer.
//!
//! ## Deliberate deviations
//!
//! * **Discriminators.** Python repeats a `Literal[...]` per class (`role` on
//!   messages, `type` on blocks) and uses a pydantic discriminated union for
//!   `AgentMessage`. Rust uses one shared enum per discriminator plus a
//!   `deserialize_with` check on each struct field. Construction can pair the
//!   wrong enum variant with a struct; deserialization never can. Per-type
//!   zero-sized tokens would close that gap too, but need either a macro or
//!   eleven hand-written impls, and the port avoids macros on purpose.
//! * **Required `role`.** Python defaults `role` per class on standalone
//!   validation, yet its `AgentMessage` union rejects a missing discriminator.
//!   Rust models both paths with one deserializer, so it picked the union
//!   behavior: `role` is required. The wire always writes it, and requiring it
//!   stops the untagged union from silently reading a role-less object as an
//!   assistant message.
//! * **No `content="..."` convenience.** Python's `WireModel` before-validators
//!   accepted a bare string for assistant and tool-result content. Rust
//!   construction is statically typed and uses [`assistant_content`] plus
//!   `..Default::default()`; string content in legacy sessions is converted by
//!   the session migration layer, which is where Python converts it as well.
//! * **Content `type` lives on the block; parent enums are untagged.** Python
//!   keeps `type` on each block class, and blocks are sometimes standalone
//!   values (`toolcall_end.tool_call`). An earlier internally-tagged enum draft
//!   dropped the tag in exactly that case, so the tag stays on the struct and
//!   the enums dispatch untagged.
//! * **`content_text` is not duck-typed.** Python's helper takes
//!   `str | list[Any]` and filters at runtime; the Rust version takes
//!   `&UserContent`, and owners of other block lists extract text through their
//!   own `text()` methods.
//! * **`MessageRole`/`ContentType` string mapping is hand-written and
//!   drift-tested**, because serde renames cannot be queried at runtime; Python
//!   simply interpolates the `Literal` value.
//! * **Rust integer types** (`u64` counts, `usize` content indices, `i32` exit
//!   codes) replace Python's unbounded `int`, rejecting malformed negative or
//!   overflowing values statically instead of at a use site.

use std::fmt;

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
///
/// Python spells this as `Literal["user"]` etc. on each class and validates
/// the `AgentMessage` union by that tag. Rust keeps one closed enum and lets
/// each struct field check the variant it expects (see [`expect_role`]); the
/// module header explains the construction-time trade-off.
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

impl MessageRole {
    /// The wire spelling of this role.
    ///
    /// serde's rename table is not queryable at runtime, so the mapping is
    /// repeated here and `display_and_serde_use_the_same_spelling` fails if the
    /// two ever drift. Python never needs this because the `Literal` value is
    /// already the string, and it uses that string directly in f-strings.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::ToolResult => "toolResult",
            Self::BashExecution => "bashExecution",
            Self::Custom => "custom",
            Self::BranchSummary => "branchSummary",
            Self::CompactionSummary => "compactionSummary",
        }
    }
}

impl fmt::Display for MessageRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Reject a role that does not belong to the message struct being deserialized.
///
/// This check is what makes the untagged [`AgentMessage`] union work: each
/// variant tries to deserialize, the role check rejects all the wrong kinds, and
/// the matching one wins. Python gets the same dispatch from pydantic's
/// `Field(discriminator="role")`.
///
/// The field has no default, so `role` is mandatory. Python's per-class default
/// only applies to standalone validation; its union also rejects a missing
/// discriminator, and the wire always writes `role`.
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
    // `WireModel` validates by alias *and* by name, so Python accepts both
    // `cacheRead` and `cache_read`. serde only knows the one rename target, so
    // the field repeats the Python name as an alias. The same pattern applies
    // to every multi-word field in this file.
    #[serde(alias = "cache_read")]
    pub(crate) cache_read: f64,
    #[serde(alias = "cache_write")]
    pub(crate) cache_write: f64,
    pub(crate) total: f64,
}

/// Provider-reported token usage for one assistant response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Usage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    #[serde(alias = "cache_read")]
    pub(crate) cache_read: u64,
    #[serde(alias = "cache_write")]
    pub(crate) cache_write: u64,
    // Python builds aliases with `_to_camel`, whose `str.title()` turns
    // `cache_write_1h` into `cacheWrite1H` (capital H); serde's own camelCase
    // would emit `cacheWrite1h`. The wire name is spelled out, and the Python
    // field name stays accepted as an alias.
    #[serde(rename = "cacheWrite1H", alias = "cache_write_1h")]
    pub(crate) cache_write_1h: Option<u64>,
    pub(crate) reasoning: Option<u64>,
    #[serde(alias = "total_tokens")]
    pub(crate) total_tokens: u64,
    pub(crate) cost: UsageCost,
}

/// Return the field-wise total for one or more provider requests.
///
/// Python accepts any `Iterable[Usage]`; the Rust slice covers every call site
/// and keeps the single pass per field. The optional fields keep the Python
/// rule: sum the values that are present, and stay `None` only when every input
/// is `None` (an empty list therefore totals to zeros and `None`).
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
    #[serde(alias = "time_to_first_output_ms")]
    pub(crate) time_to_first_output_ms: Option<u64>,
    #[serde(alias = "total_duration_ms")]
    pub(crate) total_duration_ms: u64,
}

/// The `type` discriminator shared by content blocks.
///
/// Same shape as [`MessageRole`]: Python repeats `Literal["text"]` on every
/// block class, Rust uses one enum plus [`expect_content_type`] on each field.
/// The `default_*_type` functions below reproduce Python's `= "text"` defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ContentType {
    Text,
    Thinking,
    Image,
    ToolCall,
}

impl ContentType {
    /// The wire spelling of this content type; see [`MessageRole::as_str`] for
    /// why the mapping is hand-written and drift-tested.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Thinking => "thinking",
            Self::Image => "image",
            Self::ToolCall => "toolCall",
        }
    }
}

impl fmt::Display for ContentType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Reject a content type that does not belong to the block being deserialized.
fn expect_content_type<'de, D: Deserializer<'de>>(
    deserializer: D,
    expected: ContentType,
) -> Result<ContentType, D::Error> {
    let content_type = ContentType::deserialize(deserializer)?;
    if content_type == expected {
        Ok(content_type)
    } else {
        Err(serde::de::Error::custom(format_args!(
            "expected {expected:?} content, got {content_type:?}"
        )))
    }
}

fn text_type<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ContentType, D::Error> {
    expect_content_type(deserializer, ContentType::Text)
}

fn thinking_type<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ContentType, D::Error> {
    expect_content_type(deserializer, ContentType::Thinking)
}

fn image_type<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ContentType, D::Error> {
    expect_content_type(deserializer, ContentType::Image)
}

fn tool_call_type<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ContentType, D::Error> {
    expect_content_type(deserializer, ContentType::ToolCall)
}

// Python declares each block `type` as `Literal[...] = "..."`, so a block that
// omits the key still validates and receives the default. `#[serde(default)]`
// cannot express a non-`Default` value and `deserialize_with` alone would make
// the field required, so each block pairs the two attributes with one of these
// functions.
fn default_text_type() -> ContentType {
    ContentType::Text
}

fn default_thinking_type() -> ContentType {
    ContentType::Thinking
}

fn default_image_type() -> ContentType {
    ContentType::Image
}

fn default_tool_call_type() -> ContentType {
    ContentType::ToolCall
}

// --- Content blocks ---------------------------------------------------------
//
// Python defines each block as its own class with `type: Literal[...] = "..."`,
// and the three unions (`UserContentItem`, `AssistantContent`,
// `ToolResultContent`) are plain smart unions, not discriminated ones.
//
// Rust mirrors that layout: the `type` key lives on the struct, each struct
// validates the variant it expects, and the parent enums are `#[serde(untagged)]`.
// An earlier draft put `#[serde(tag = "type")]` on the enums, which is the
// idiomatic Rust shape, but it broke `toolcall_end.tool_call`: that field holds a
// `ToolCall` on its own, and Python's class always writes `"type":"toolCall"`.
// With the tag on the enum, the standalone value lost its tag entirely.
//
// `Default` is intentionally not derived for the block structs: three of the four
// would get the wrong `type` from a shared default, and Python's defaults are
// per-class. The `new` constructors pin the correct tag instead.
// ---------------------------------------------------------------------------

/// A plain text content block.
///
/// Field order matches Python, and `r#type` serializes first, so a block prints
/// exactly like the Python class at every nesting depth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextContent {
    // `type` is a Rust keyword; the raw identifier plus `rename_all` keeps the
    // wire key `type`, and the deserializer rejects every other content type.
    #[serde(deserialize_with = "text_type", default = "default_text_type")]
    pub(crate) r#type: ContentType,
    pub(crate) text: String,
    #[serde(alias = "text_signature")]
    pub(crate) text_signature: Option<String>,
}

impl TextContent {
    /// Build a text block with the fixed `type` and Python's field defaults.
    ///
    /// Rust structs have no field defaults, so constructors replace Python's
    /// keyword-argument style; direct struct literals remain possible and must
    /// spell `r#type` out.
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self {
            r#type: ContentType::Text,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingContent {
    #[serde(deserialize_with = "thinking_type", default = "default_thinking_type")]
    pub(crate) r#type: ContentType,
    pub(crate) thinking: String,
    #[serde(alias = "thinking_signature")]
    pub(crate) thinking_signature: Option<String>,
    #[serde(default)]
    pub(crate) redacted: bool,
}

impl ThinkingContent {
    pub(crate) fn new(thinking: impl Into<String>) -> Self {
        Self {
            r#type: ContentType::Thinking,
            thinking: thinking.into(),
            thinking_signature: None,
            redacted: false,
        }
    }
}

/// A base64 image content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ImageContent {
    #[serde(deserialize_with = "image_type", default = "default_image_type")]
    pub(crate) r#type: ContentType,
    pub(crate) data: String,
    #[serde(alias = "mime_type")]
    pub(crate) mime_type: String,
}

impl ImageContent {
    pub(crate) fn new(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            r#type: ContentType::Image,
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }
}

/// A tool call content block requested by the assistant.
///
/// This is the block that forced `type` onto the structs: `toolcall_end`
/// serializes a `ToolCall` as a plain field outside any content list, and
/// Python's class writes `"type":"toolCall"` there as well.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCall {
    #[serde(
        deserialize_with = "tool_call_type",
        default = "default_tool_call_type"
    )]
    pub(crate) r#type: ContentType,
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) arguments: JsonObject,
    #[serde(alias = "thought_signature")]
    pub(crate) thought_signature: Option<String>,
}

impl ToolCall {
    pub(crate) fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            r#type: ContentType::ToolCall,
            id: id.into(),
            name: name.into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        }
    }
}

/// One block of a user message: text or image.
///
/// Untagged because the block structs carry and validate the `type` tag
/// themselves; see the content-block notes above. Dispatch stays unambiguous
/// because the payload keys (`text` vs `data`/`mimeType`) differ.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum UserContentItem {
    Text(TextContent),
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

// Python's union types coerce implicitly at runtime (`str | list[...]`). Rust
// spells the useful coercions out as `From`, so call sites can write
// `content: "hello".into()`. Only one `From` exists per target type to keep
// `.into()` inference unambiguous.
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

/// An ordered assistant content block: text, thinking, or tool call.
///
/// Untagged for the same reason as [`UserContentItem`]. Order is preserved by
/// the `Vec` on the message, matching the Python list of blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AssistantContent {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCall),
}

/// One block of a tool result: text or image.
///
/// Untagged for the same reason as [`UserContentItem`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum ToolResultContent {
    Text(TextContent),
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

/// Structured provider error code, Python's `str | int | None`.
///
/// Variant order matters: serde tries the string variant first, so JSON strings
/// stay strings and numbers fall through to the integer variant, matching
/// pydantic's smart union. Python allows unbounded and negative integers, so
/// the Rust variant is `i64` rather than `u64`.
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
    // Python names the field `type`; the raw identifier mirrors that name, and
    // `rename` keeps the wire key explicit alongside `rename_all`.
    #[serde(rename = "type")]
    pub(crate) r#type: String,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
    pub(crate) error: Option<AssistantDiagnosticError>,
    pub(crate) details: Option<JsonObject>,
}

/// Why the provider stopped generating an assistant response.
///
/// Python's `StopReason = Literal["stop", "length", "toolUse", "error",
/// "aborted"]`; the enum keeps the same wire values and lets the compiler check
/// matches. `Default` mirrors Python's `stop_reason="stop"`.
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
///
/// The container deliberately has no blanket `#[serde(default)]`: that would
/// also default the required `role` and let [`AgentMessage`] misread role-less
/// input as an assistant message. Python's defaults are reproduced field by
/// field instead (`content=[]`, `api/provider/model="unknown"`,
/// `usage=Usage()`, `stop_reason="stop"`, `timestamp=now`), and `usage`
/// additionally accepts an explicit `null` through [`deserialize_null_default`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantMessage {
    #[serde(deserialize_with = "assistant_role")]
    pub(crate) role: MessageRole,
    #[serde(default)]
    pub(crate) content: Vec<AssistantContent>,
    #[serde(default = "unknown")]
    pub(crate) api: String,
    #[serde(default = "unknown")]
    pub(crate) provider: String,
    #[serde(default = "unknown")]
    pub(crate) model: String,
    #[serde(alias = "response_model")]
    pub(crate) response_model: Option<String>,
    #[serde(alias = "response_provider")]
    pub(crate) response_provider: Option<String>,
    #[serde(alias = "response_id")]
    pub(crate) response_id: Option<String>,
    pub(crate) diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub(crate) usage: Usage,
    pub(crate) timing: Option<ResponseTiming>,
    #[serde(default, alias = "stop_reason")]
    pub(crate) stop_reason: StopReason,
    #[serde(alias = "error_message")]
    pub(crate) error_message: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

/// Construction helper with Python's defaults.
///
/// Deserialization fills the same values field by field (see the struct doc);
/// this impl exists so Rust call sites can use `..Default::default()` instead
/// of repeating `api="unknown"`, `usage=Usage()`, and the rest.
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
    #[serde(alias = "tool_call_id")]
    pub(crate) tool_call_id: String,
    #[serde(alias = "tool_name")]
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) content: Vec<ToolResultContent>,
    pub(crate) details: Option<JsonValue>,
    #[serde(alias = "added_tool_names")]
    pub(crate) added_tool_names: Option<Vec<String>>,
    #[serde(default, alias = "is_error")]
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
    // Python's `int | None`. `i32` covers every process exit status, including
    // the negative codes shells report for signal deaths, and rejects the
    // unbounded values Python would happily store.
    #[serde(alias = "exit_code")]
    pub(crate) exit_code: Option<i32>,
    #[serde(default)]
    pub(crate) cancelled: bool,
    #[serde(default)]
    pub(crate) truncated: bool,
    #[serde(alias = "full_output_path")]
    pub(crate) full_output_path: Option<String>,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
    #[serde(default, alias = "exclude_from_context")]
    pub(crate) exclude_from_context: bool,
}

/// An extension-injected message that participates in model context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct CustomMessage {
    #[serde(deserialize_with = "custom_role")]
    pub(crate) role: MessageRole,
    #[serde(alias = "custom_type")]
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
    #[serde(alias = "from_id")]
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
    #[serde(alias = "tokens_before")]
    pub(crate) tokens_before: u64,
    #[serde(default = "current_timestamp_ms")]
    pub(crate) timestamp: u64,
}

/// Any transcript message.
///
/// Python's `AgentMessage` is a discriminated union on `role`. Rust keeps the
/// `role` key inside each struct, because provider events embed concrete
/// messages as plain fields (`partial`/`message`/`error`) and those need the key
/// when serialized on their own. The enum is therefore `untagged`, and the
/// per-struct role check picks the variant.
///
/// Trade-off: malformed input produces serde's coarse "data did not match any
/// variant" error instead of pydantic's per-variant report. Reading `role` from
/// the raw value first is the escape hatch for callers that want better
/// diagnostics. A manually tagged enum would need role-less structs plus a
/// second, tag-only representation purely for the embedded cases.
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

// Stand-ins for Python's implicit union coercion at call sites: Python accepts
// any of the message classes wherever `AgentMessage` is expected, Rust asks for
// an explicit `.into()`.
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
    ///
    /// Python reads the `role` attribute directly. An enum cannot expose a
    /// common field, so the accessor matches every variant; the compiler then
    /// keeps it in sync when a message type is added.
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
///
/// Python's `tool_calls` parameter defaults to `()`. Rust has no default
/// arguments, so the parameter is required; it accepts any iterator, which
/// makes `assistant_content(text, [])` read close to the Python call. The
/// `content=str` acceptance of Python's before-validator is intentionally not
/// part of deserialization: legacy strings are converted by the session
/// migration layer.
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
///
/// Python's `content_text` is duck-typed (`str | list[Any]`) and filters with
/// `isinstance(block, TextContent)`. The Rust version takes `&UserContent`, the
/// type Python callers actually pass, and owners of other block lists extract
/// text through their own methods (`ToolResultMessage::text` maps
/// [`ToolResultContent::text`]). That removes the runtime type check without
/// duplicating the export.
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
/// Python dispatches with `isinstance` and falls through to `""`. The Rust
/// enum is exhaustive, so every case is spelled out, the compiler forces a new
/// message type to be handled, and no malformed message can silently produce an
/// empty preview.
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
                UserContentItem::Image(ImageContent::new("b64", "image/png")),
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
                    redacted: true,
                    ..ThinkingContent::new("t")
                }),
                AssistantContent::Text(TextContent {
                    text_signature: Some("sig".into()),
                    ..TextContent::new("a")
                }),
                AssistantContent::ToolCall(ToolCall {
                    arguments: JsonObject::from_iter([("path".to_owned(), json!("x"))]),
                    thought_signature: Some("ts".into()),
                    ..ToolCall::new("c1", "read")
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
        let call = ToolCall::new("c1", "read");
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
                AssistantContent::Thinking(ThinkingContent::new("why")),
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
            ToolResultContent::Image(ImageContent::new("b64", "image/png")),
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
                UserContentItem::Image(ImageContent::new("b64", "image/png")),
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
        let call = ToolCall::new("c1", "read");

        assert_eq!(
            assistant_content("hello", [call.clone()]),
            vec![
                AssistantContent::Text(TextContent::new("hello")),
                AssistantContent::ToolCall(call),
            ]
        );
        assert!(assistant_content("", Vec::<ToolCall>::new()).is_empty());
    }

    #[test]
    fn agent_message_requires_a_role() {
        // Python's discriminated union rejects a missing discriminator; the
        // untagged Rust union must not fall back to the assistant variant.
        assert!(serde_json::from_str::<AgentMessage>(r#"{"content":[]}"#).is_err());
        assert!(serde_json::from_str::<AgentMessage>(r#"{"content":"x"}"#).is_err());
    }

    #[test]
    fn content_blocks_default_their_type() {
        let message: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","content":[{"text":"a"},{"thinking":"t"},{"id":"c1","name":"read"}]}"#,
        )
        .unwrap();
        assert_eq!(
            message.content,
            vec![
                AssistantContent::Text(TextContent::new("a")),
                AssistantContent::Thinking(ThinkingContent::new("t")),
                AssistantContent::ToolCall(ToolCall::new("c1", "read")),
            ]
        );

        let message: UserMessage = serde_json::from_str(
            r#"{"role":"user","content":[{"data":"b64","mimeType":"image/png"}]}"#,
        )
        .unwrap();
        assert_eq!(
            message.content,
            UserContent::List(vec![UserContentItem::Image(ImageContent::new(
                "b64",
                "image/png"
            ))])
        );
    }

    #[test]
    fn python_snake_case_field_names_are_accepted() {
        // Python's WireModel sets validate_by_name=True; the legacy session
        // loader relies on it (for example usage.cache_read).
        let usage: Usage = serde_json::from_str(
            r#"{"cache_read":1,"cache_write":2,"cache_write_1h":3,"total_tokens":4}"#,
        )
        .unwrap();
        assert_eq!(usage.cache_read, 1);
        assert_eq!(usage.cache_write, 2);
        assert_eq!(usage.cache_write_1h, Some(3));
        assert_eq!(usage.total_tokens, 4);

        let assistant: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","stop_reason":"length","response_model":"m","error_message":"e"}"#,
        )
        .unwrap();
        assert_eq!(assistant.stop_reason, StopReason::Length);
        assert_eq!(assistant.response_model.as_deref(), Some("m"));
        assert_eq!(assistant.error_message.as_deref(), Some("e"));

        let tool_result: ToolResultMessage = serde_json::from_str(
            r#"{"role":"toolResult","tool_call_id":"c1","tool_name":"read","is_error":true,"added_tool_names":["x"]}"#,
        )
        .unwrap();
        assert_eq!(tool_result.tool_call_id, "c1");
        assert_eq!(tool_result.tool_name, "read");
        assert!(tool_result.is_error);
        assert_eq!(tool_result.added_tool_names, Some(vec!["x".to_owned()]));

        let text: TextContent =
            serde_json::from_str(r#"{"text":"a","text_signature":"sig"}"#).unwrap();
        assert_eq!(text.text_signature.as_deref(), Some("sig"));

        // Python lets the alias win when both spellings are present; serde
        // rejects the duplicate instead. Stricter, and wire data never
        // contains both spellings of one field.
        assert!(serde_json::from_str::<Usage>(r#"{"cacheRead":1,"cache_read":2}"#).is_err());
    }

    #[test]
    fn agent_message_matches_python_acceptance_corpus() {
        // Verdicts produced by TypeAdapter(AgentMessage).validate_python in
        // Python. This pins the accept/reject contract of the union.
        let cases: [(&str, bool); 28] = [
            (r#"{"role":"user","content":"x"}"#, true),
            (r#"{"role":"user","content":[]}"#, true),
            (
                r#"{"role":"user","content":[{"type":"text","text":"x"}]}"#,
                true,
            ),
            (r#"{"role":"user","content":[{"text":"x"}]}"#, true),
            (
                r#"{"role":"user","content":[{"type":"image","text":"x"}]}"#,
                false,
            ),
            (
                r#"{"role":"user","content":[{"type":"zzz","text":"x"}]}"#,
                false,
            ),
            (r#"{"role":"user"}"#, false),
            (r#"{"role":"user","content":"x","bogus":1}"#, false),
            (r#"{"role":"assistant"}"#, true),
            (
                r#"{"role":"assistant","content":[{"id":"c","name":"n"}]}"#,
                true,
            ),
            (
                r#"{"role":"assistant","content":[{"data":"x","mimeType":"m"}]}"#,
                false,
            ),
            (r#"{"role":"assistant","usage":null}"#, true),
            (
                r#"{"role":"assistant","usage":{"cache_read":1,"total_tokens":2}}"#,
                true,
            ),
            (r#"{"role":"assistant","stopReason":"toolUse"}"#, true),
            (r#"{"role":"assistant","stopReason":"nope"}"#, false),
            (r#"{"role":"assistant","stop_reason":"toolUse"}"#, true),
            (
                r#"{"role":"toolResult","toolCallId":"c","toolName":"t"}"#,
                true,
            ),
            (
                r#"{"role":"toolResult","tool_call_id":"c","tool_name":"t"}"#,
                true,
            ),
            (
                r#"{"role":"toolResult","toolCallId":"c","toolName":"t","isError":true}"#,
                true,
            ),
            (
                r#"{"role":"bashExecution","command":"ls","output":"o"}"#,
                true,
            ),
            (r#"{"role":"bashExecution","command":"ls"}"#, false),
            (r#"{"role":"custom","customType":"ct","content":"x"}"#, true),
            (r#"{"role":"custom","content":"x"}"#, false),
            (
                r#"{"role":"branchSummary","summary":"s","fromId":"f"}"#,
                true,
            ),
            (
                r#"{"role":"compactionSummary","summary":"s","tokensBefore":3}"#,
                true,
            ),
            (r#"{"content":"x"}"#, false),
            (r#"{"role":"nope","content":"x"}"#, false),
            (
                r#"{"role":"assistant","content":[{"thinking":"t","redacted":true}]}"#,
                true,
            ),
        ];

        for (payload, accepted) in cases {
            assert_eq!(
                serde_json::from_str::<AgentMessage>(payload).is_ok(),
                accepted,
                "payload: {payload}"
            );
        }
    }

    #[test]
    fn display_and_serde_use_the_same_spelling() {
        for role in [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::ToolResult,
            MessageRole::BashExecution,
            MessageRole::Custom,
            MessageRole::BranchSummary,
            MessageRole::CompactionSummary,
        ] {
            assert_eq!(serde_json::to_string(&role).unwrap(), format!("\"{role}\""));
        }

        for content_type in [
            ContentType::Text,
            ContentType::Thinking,
            ContentType::Image,
            ContentType::ToolCall,
        ] {
            assert_eq!(
                serde_json::to_string(&content_type).unwrap(),
                format!("\"{content_type}\"")
            );
        }
    }

    #[test]
    fn json_objects_preserve_insertion_order() {
        // The serde_json `preserve_order` feature keeps `arguments` in the
        // order a provider or a Python dict wrote it.
        let mut arguments = JsonObject::new();
        arguments.insert("z".to_owned(), json!(1));
        arguments.insert("a".to_owned(), json!(2));
        let call = ToolCall {
            arguments,
            ..ToolCall::new("c1", "read")
        };

        assert!(
            serde_json::to_string(&call)
                .unwrap()
                .contains(r#""arguments":{"z":1,"a":2}"#)
        );
    }

    /// One value of every message variant, shared by the coverage tests below.
    fn sample_messages() -> Vec<AgentMessage> {
        vec![
            UserMessage {
                role: MessageRole::User,
                content: "prompt".into(),
                timestamp: 9,
            }
            .into(),
            AssistantMessage {
                content: vec![
                    AssistantContent::Text(TextContent::new("part")),
                    AssistantContent::ToolCall(ToolCall::new("c1", "read")),
                ],
                timestamp: 9,
                ..Default::default()
            }
            .into(),
            ToolResultMessage {
                role: MessageRole::ToolResult,
                tool_call_id: "c1".into(),
                tool_name: "read".into(),
                content: vec![ToolResultContent::Text(TextContent::new("out"))],
                details: None,
                added_tool_names: None,
                is_error: false,
                timestamp: 9,
            }
            .into(),
            BashExecutionMessage {
                role: MessageRole::BashExecution,
                command: "ls".into(),
                output: "o".into(),
                exit_code: Some(0),
                cancelled: false,
                truncated: false,
                full_output_path: None,
                timestamp: 9,
                exclude_from_context: false,
            }
            .into(),
            CustomMessage {
                role: MessageRole::Custom,
                custom_type: "note".into(),
                content: "x".into(),
                display: true,
                details: None,
                timestamp: 9,
            }
            .into(),
            BranchSummaryMessage {
                role: MessageRole::BranchSummary,
                summary: "branch summary".into(),
                from_id: "f".into(),
                timestamp: 9,
            }
            .into(),
            CompactionSummaryMessage {
                role: MessageRole::CompactionSummary,
                summary: "compaction summary".into(),
                tokens_before: 3,
                timestamp: 9,
            }
            .into(),
        ]
    }

    #[test]
    fn every_message_variant_round_trips_through_the_union() {
        let expected_roles = [
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::ToolResult,
            MessageRole::BashExecution,
            MessageRole::Custom,
            MessageRole::BranchSummary,
            MessageRole::CompactionSummary,
        ];

        for (message, role) in sample_messages().iter().zip(expected_roles) {
            assert_eq!(message.role(), role);
            let json = serde_json::to_string(message).unwrap();
            let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(&parsed, message);
        }
    }

    #[test]
    fn message_text_and_timestamp_cover_every_message_role() {
        let expected_texts = [
            "prompt",
            "part",
            "out",
            "o",
            "x",
            "branch summary",
            "compaction summary",
        ];

        for (message, text) in sample_messages().iter().zip(expected_texts) {
            assert_eq!(message_text(message), text);
            assert_eq!(message.text(), text);
            assert_eq!(message.timestamp(), 9);
        }
    }

    #[test]
    fn message_to_user_covers_every_message_role() {
        let expected_texts = [
            "prompt",
            "part",
            "out",
            "o",
            "x",
            "branch summary",
            "compaction summary",
        ];

        for (message, text) in sample_messages().iter().zip(expected_texts) {
            let user = message_to_user(message);
            assert_eq!(user.role, MessageRole::User);
            assert_eq!(user.timestamp, 9);
            assert_eq!(user.text(), text);
        }
    }

    #[test]
    fn standalone_content_blocks_keep_their_type_tag() {
        // Locks the ADR-004 decision: a block serialized outside a content list
        // still writes its `type`, exactly like the Python class.
        assert_eq!(
            serde_json::to_string(&TextContent::new("a")).unwrap(),
            r#"{"type":"text","text":"a","textSignature":null}"#
        );
        assert_eq!(
            serde_json::to_string(&ThinkingContent::new("t")).unwrap(),
            r#"{"type":"thinking","thinking":"t","thinkingSignature":null,"redacted":false}"#
        );
        assert_eq!(
            serde_json::to_string(&ImageContent::new("d", "image/png")).unwrap(),
            r#"{"type":"image","data":"d","mimeType":"image/png"}"#
        );
        assert_eq!(
            serde_json::to_string(&ToolCall::new("c", "n")).unwrap(),
            r#"{"type":"toolCall","id":"c","name":"n","arguments":{},"thoughtSignature":null}"#
        );
    }

    #[test]
    fn remaining_snake_case_aliases_are_accepted() {
        let assistant: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","content":[{"type":"thinking","thinking":"t","thinking_signature":"sig"},{"type":"toolCall","id":"c","name":"n","thought_signature":"ts"}],"timing":{"time_to_first_output_ms":1,"total_duration_ms":2},"usage":{"cost":{"cache_read":0.5,"cache_write":0.25}}}"#,
        )
        .unwrap();
        let AssistantContent::Thinking(thinking) = &assistant.content[0] else {
            panic!("expected thinking block");
        };
        assert_eq!(thinking.thinking_signature.as_deref(), Some("sig"));
        let AssistantContent::ToolCall(call) = &assistant.content[1] else {
            panic!("expected tool call block");
        };
        assert_eq!(call.thought_signature.as_deref(), Some("ts"));
        let timing = assistant.timing.unwrap();
        assert_eq!(timing.time_to_first_output_ms, Some(1));
        assert_eq!(timing.total_duration_ms, 2);
        assert_eq!(assistant.usage.cost.cache_read, 0.5);
        assert_eq!(assistant.usage.cost.cache_write, 0.25);

        let user: UserMessage = serde_json::from_str(
            r#"{"role":"user","content":[{"type":"image","data":"d","mime_type":"image/png"}]}"#,
        )
        .unwrap();
        let UserContent::List(items) = &user.content else {
            panic!("expected an image list");
        };
        let UserContentItem::Image(image) = &items[0] else {
            panic!("expected an image block");
        };
        assert_eq!(image.mime_type, "image/png");

        let bash: BashExecutionMessage = serde_json::from_str(
            r#"{"role":"bashExecution","command":"ls","output":"o","exit_code":-1,"full_output_path":"/tmp/x","exclude_from_context":true}"#,
        )
        .unwrap();
        assert_eq!(bash.exit_code, Some(-1));
        assert_eq!(bash.full_output_path.as_deref(), Some("/tmp/x"));
        assert!(bash.exclude_from_context);

        let custom: CustomMessage =
            serde_json::from_str(r#"{"role":"custom","custom_type":"ct","content":"x"}"#).unwrap();
        assert_eq!(custom.custom_type, "ct");

        let branch: BranchSummaryMessage =
            serde_json::from_str(r#"{"role":"branchSummary","summary":"s","from_id":"f"}"#)
                .unwrap();
        assert_eq!(branch.from_id, "f");

        let compaction: CompactionSummaryMessage =
            serde_json::from_str(r#"{"role":"compactionSummary","summary":"s","tokens_before":3}"#)
                .unwrap();
        assert_eq!(compaction.tokens_before, 3);
    }

    #[test]
    fn diagnostic_error_code_accepts_strings_and_integers() {
        let message: AssistantMessage = serde_json::from_str(
            r#"{"role":"assistant","diagnostics":[{"type":"w","error":{"message":"m","code":"E1"}},{"type":"w","error":{"message":"m","code":-7}}]}"#,
        )
        .unwrap();
        let diagnostics = message.diagnostics.unwrap();
        assert_eq!(
            diagnostics[0].error.as_ref().unwrap().code,
            Some(AssistantDiagnosticErrorCode::Str("E1".into()))
        );
        assert_eq!(
            diagnostics[1].error.as_ref().unwrap().code,
            Some(AssistantDiagnosticErrorCode::Int(-7))
        );

        // Objects and booleans belong to neither variant.
        assert!(
            serde_json::from_str::<AssistantMessage>(
                r#"{"role":"assistant","diagnostics":[{"type":"w","error":{"message":"m","code":{}}}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn stop_reason_covers_python_literals() {
        let cases = [
            ("stop", StopReason::Stop),
            ("length", StopReason::Length),
            ("toolUse", StopReason::ToolUse),
            ("error", StopReason::Error),
            ("aborted", StopReason::Aborted),
        ];

        for (wire, expected) in cases {
            let message: AssistantMessage =
                serde_json::from_str(&format!(r#"{{"role":"assistant","stopReason":"{wire}"}}"#))
                    .unwrap();
            assert_eq!(message.stop_reason, expected);
            assert_eq!(serde_json::to_value(expected).unwrap(), json!(wire));
        }

        assert!(
            serde_json::from_str::<AssistantMessage>(
                r#"{"role":"assistant","stopReason":"nope"}"
            )
            .is_err()
        );
    }

    #[test]
    fn unsigned_fields_reject_negative_values_and_exit_code_is_signed() {
        assert!(
            serde_json::from_str::<AgentMessage>(r#"{"role":"user","content":"x","timestamp":-1}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<AgentMessage>(
                r#"{"role":"compactionSummary","summary":"s","tokensBefore":-1}"
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<AgentMessage>(r#"{"role":"assistant","usage":{"input":-1}}"#
            )
            .is_err()
        );

        let bash: BashExecutionMessage = serde_json::from_str(
            r#"{"role":"bashExecution","command":"c","output":"o","exitCode":-9}"#,
        )
        .unwrap();
        assert_eq!(bash.exit_code, Some(-9));
    }

    #[test]
    fn string_content_is_rejected_and_left_to_the_migration_layer() {
        assert!(
            serde_json::from_str::<AssistantMessage>(r#"{"role":"assistant","content":"hi"}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ToolResultMessage>(
                r#"{"role":"toolResult","toolCallId":"c","toolName":"t","content":"hi"}"#
            )
            .is_err()
        );
    }

    #[test]
    fn sum_usage_sums_cost_fields() {
        let total = sum_usage(&[
            Usage {
                input: 1,
                output: 2,
                cache_read: 3,
                cache_write: 4,
                total_tokens: 5,
                cost: UsageCost {
                    input: 0.5,
                    output: 0.25,
                    cache_read: 0.125,
                    cache_write: 0.0625,
                    total: 1.0,
                },
                ..Usage::default()
            },
            Usage {
                input: 10,
                cost: UsageCost {
                    input: 1.5,
                    output: 0.75,
                    cache_read: 0.25,
                    cache_write: 0.125,
                    total: 2.0,
                },
                ..Usage::default()
            },
        ]);

        assert_eq!(total.input, 11);
        assert_eq!(total.output, 2);
        assert_eq!(total.cache_read, 3);
        assert_eq!(total.cache_write, 4);
        assert_eq!(total.total_tokens, 5);
        assert_eq!(total.cost.input, 2.0);
        assert_eq!(total.cost.output, 1.0);
        assert_eq!(total.cost.cache_read, 0.375);
        assert_eq!(total.cost.cache_write, 0.1875);
        assert_eq!(total.cost.total, 3.0);
    }

    #[test]
    fn assistant_accessors_preserve_content_order() {
        let message = AssistantMessage {
            content: vec![
                AssistantContent::ToolCall(ToolCall::new("c1", "read")),
                AssistantContent::Text(TextContent::new("a")),
                AssistantContent::Thinking(ThinkingContent::new("why")),
                AssistantContent::ToolCall(ToolCall::new("c2", "write")),
                AssistantContent::Text(TextContent::new("b")),
            ],
            timestamp: 1,
            ..Default::default()
        };

        assert_eq!(message.text(), "ab");
        assert_eq!(message.thinking_text(), "why");
        assert_eq!(
            message
                .tool_calls()
                .iter()
                .map(|call| call.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c1", "c2"]
        );
    }

    #[test]
    fn user_content_conversions_and_empty_list() {
        assert_eq!(UserContent::from("x"), UserContent::Str("x".to_owned()));
        assert_eq!(
            UserContent::from("x".to_owned()),
            UserContent::Str("x".to_owned())
        );

        let empty: UserContent = Vec::<UserContentItem>::new().into();
        let message = UserMessage {
            role: MessageRole::User,
            content: empty,
            timestamp: 1,
        };
        assert_eq!(
            serde_json::to_string(&message).unwrap(),
            r#"{"role":"user","content":[],"timestamp":1}"#
        );

        let blocks: UserContent =
            vec![UserContentItem::Image(ImageContent::new("d", "image/png"))].into();
        assert_eq!(
            serde_json::to_string(&blocks).unwrap(),
            r#"[{"type":"image","data":"d","mimeType":"image/png"}]"#
        );
    }
}
