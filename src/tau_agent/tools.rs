//! Pi-compatible provider-neutral tool definitions and execution results.
//!
//! Rust port of `tau_agent/tools.py`. The wire shape of [`AgentToolResult`]
//! matches the Python model; this header collects the deliberate deviations:
//!
//! * **`Protocol` / `Callable` become boxed closures.** Python's renderers,
//!   executor, update callback, and argument preparer are structural callables
//!   that tools and extensions supply as lambdas. Rust keeps the Python names
//!   as type aliases over `Box<dyn Fn...>` so call sites still pass closures.
//! * **The executor returns a boxed `Future`.** Python's `ToolExecutor` returns
//!   an awaitable. Rust stores it as `Box<dyn Fn(...) -> ToolFuture>` built on
//!   `std::future::Future`, which `futures_core::Future` re-exports; the
//!   portable layer stays free of an async runtime (ADR-010).
//! * **The update callback is `FnMut` + `'static`.** Python's callback closes
//!   over a local list; Rust callers share state through `Arc<Mutex<...>>` (or
//!   a channel) because the boxed callback may outlive the call site.
//! * **`content="..."` is not accepted.** Python's before-validator rewrites a
//!   bare string; per ADR-006 that convenience belongs to the migration layer,
//!   not to the wire model.
//! * **`execution_mode` is an enum** instead of `Literal["sequential",
//!   "parallel"]`; it is not serialized (Python's dataclass is not a
//!   `WireModel` either).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::messages::ToolResultContent;
use super::types::{JsonObject, JsonValue};

/// Return whether tool execution should stop.
pub(crate) trait ToolCancellationToken: Send + Sync {
    fn is_cancelled(&self) -> bool;
}

/// Final or partial result produced by a tool.
///
/// A wire model: Python nests it in `tool_execution_update` and
/// `tool_execution_end` events, so it serializes with camelCase keys, the
/// Python field names as aliases, and pydantic's defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AgentToolResult {
    #[serde(default)]
    pub(crate) content: Vec<ToolResultContent>,
    pub(crate) details: Option<JsonValue>,
    #[serde(alias = "added_tool_names")]
    pub(crate) added_tool_names: Option<Vec<String>>,
    pub(crate) terminate: Option<bool>,
}

impl AgentToolResult {
    /// Visible text of the result; images contribute nothing.
    ///
    /// Python exposes this as the `text` property.
    pub(crate) fn text(&self) -> String {
        self.content.iter().map(ToolResultContent::text).collect()
    }
}

/// Frontend renderer for a tool invocation.
pub(crate) type ToolCallRenderer = Box<dyn Fn(&JsonObject) -> Option<String> + Send + Sync>;

/// Frontend renderer for a tool result.
pub(crate) type ToolResultRenderer =
    Box<dyn Fn(&AgentToolResult, bool) -> Option<String> + Send + Sync>;

/// Progress callback a tool may call while it runs.
pub(crate) type ToolUpdateCallback = Box<dyn FnMut(AgentToolResult) + Send>;

/// Future returned by a tool executor.
pub(crate) type ToolFuture = Pin<Box<dyn Future<Output = AgentToolResult> + Send + 'static>>;

/// Execute one validated tool call.
pub(crate) type ToolExecutor = Box<
    dyn Fn(
            String,
            JsonObject,
            Option<Arc<dyn ToolCancellationToken>>,
            Option<ToolUpdateCallback>,
        ) -> ToolFuture
        + Send
        + Sync,
>;

/// How a set of tool calls may be executed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ToolExecutionMode {
    Sequential,
    #[default]
    Parallel,
}

/// Convert raw model arguments into canonical tool arguments.
pub(crate) type ToolArgumentPreparer = Box<dyn Fn(&JsonValue) -> JsonObject + Send + Sync>;

/// A tool exposed to the portable agent loop.
pub(crate) struct AgentTool {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) description: String,
    pub(crate) parameters: JsonObject,
    pub(crate) execute_fn: ToolExecutor,
    pub(crate) prompt_snippet: Option<String>,
    pub(crate) prompt_guidelines: Vec<String>,
    pub(crate) prepare_arguments: Option<ToolArgumentPreparer>,
    pub(crate) execution_mode: ToolExecutionMode,
    pub(crate) render_call: Option<ToolCallRenderer>,
    pub(crate) render_result: Option<ToolResultRenderer>,
}

impl AgentTool {
    /// Build a tool with Python's dataclass defaults.
    ///
    /// Rust structs have no field defaults, so the five required fields live
    /// here and the optional ones start at their Python defaults. Call sites
    /// extend the result with `..AgentTool::new(...)`.
    pub(crate) fn new(
        name: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        parameters: JsonObject,
        execute_fn: ToolExecutor,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            parameters,
            execute_fn,
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            prepare_arguments: None,
            execution_mode: ToolExecutionMode::default(),
            render_call: None,
            render_result: None,
        }
    }

    /// Alias used by provider payload builders.
    pub(crate) fn input_schema(&self) -> &JsonObject {
        &self.parameters
    }

    /// Execute a tool with Pi-compatible call-id and progress semantics.
    pub(crate) async fn execute(
        &self,
        tool_call_id: String,
        arguments: JsonObject,
        signal: Option<Arc<dyn ToolCancellationToken>>,
        on_update: Option<ToolUpdateCallback>,
    ) -> AgentToolResult {
        (self.execute_fn)(tool_call_id, arguments, signal, on_update).await
    }
}
