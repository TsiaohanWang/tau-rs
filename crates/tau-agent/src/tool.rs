//! Tool definitions and execution contract (the portable agent's `tools.py`).
//!
//! A tool is a typed function: a JSON-schema parameters map plus an async
//! executor returning a structured `AgentToolResult`. Mirrors
//! `tau_agent.tools.AgentTool` / `ToolExecutor`. The `AgentTool` struct holds
//! shared (Arc) handles so it is cheaply `Clone` and safely shared across the
//! loop, harness and any extension runtime.

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use tau_types::{AgentMessage, AgentToolResult};

/// Error produced by a tool executor. The agent loop converts it into a
/// canonical error result (`is_error = true`, message = the error text),
/// mirroring Python's `except Exception` isolation boundary.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ToolError {
    pub message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        ToolError {
            message: message.into(),
        }
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError::new(s)
    }
}

/// Async tool executor contract.
///
/// `on_update` carries partial progress snapshots. The loop collects them and
/// emits `ToolExecutionUpdateEvent`s after the executor returns (matching
/// Python, which batches streamed updates after completion).
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(
        &self,
        tool_call_id: &str,
        arguments: &Map<String, Value>,
        signal: Option<CancellationToken>,
        on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> Result<AgentToolResult, ToolError>;
}

pub type ToolUpdateCallback = dyn Fn(AgentToolResult) + Send + Sync;

/// Whether a tool's multiple calls may run concurrently. The agent loop does
/// **not** yet honour parallel execution — all calls run sequentially
/// (matching Python). `Parallel` is accepted but ignored for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecutionMode {
    Parallel,
    #[default]
    Sequential,
}

/// Render a tool invocation for a frontend, or `None` to use a default.
pub type ToolCallRenderer = dyn Fn(&Map<String, Value>) -> Option<String> + Send + Sync;
/// Render a tool result, or `None` to use a default. `expanded` controls detail.
pub type ToolResultRenderer = dyn Fn(&AgentToolResult, bool) -> Option<String> + Send + Sync;
/// Build normalized arguments from raw JSON (used by typed tool wrappers).
pub type ToolArgumentPreparer = dyn Fn(&Value) -> Map<String, Value> + Send + Sync;

/// Hook fired before a tool call. Returns `(blocked, reason)`: when `blocked`
/// is true the loop short-circuits with an error result carrying `reason`.
pub trait BeforeToolCall: Send + Sync {
    fn call(&self, tool_call: &tau_types::ToolCall) -> BoxFuture<'_, (bool, Option<String>)>;
}

/// Hook fired after a tool call. Returns the (possibly rewritten) result and
/// the error flag.
pub trait AfterToolCall: Send + Sync {
    fn call(
        &self,
        tool_call: &tau_types::ToolCall,
        result: AgentToolResult,
        is_error: bool,
    ) -> BoxFuture<'_, (AgentToolResult, bool)>;
}

/// A tool exposed to the portable agent loop.
#[derive(Clone)]
pub struct AgentTool {
    pub name: Arc<str>,
    pub label: String,
    pub description: String,
    pub parameters: Value,
    pub executor: Arc<dyn ToolExecutor>,
    pub prompt_snippet: Option<String>,
    pub prompt_guidelines: Vec<String>,
    pub prepare_arguments: Option<Arc<ToolArgumentPreparer>>,
    pub execution_mode: ToolExecutionMode,
    pub render_call: Option<Arc<ToolCallRenderer>>,
    pub render_result: Option<Arc<ToolResultRenderer>>,
}

impl AgentTool {
    /// Alias mirroring Python's `AgentTool.input_schema`.
    pub fn input_schema(&self) -> &Value {
        &self.parameters
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("description", &self.description)
            .field("execution_mode", &self.execution_mode)
            .finish_non_exhaustive()
    }
}

impl PartialEq for AgentTool {
    fn eq(&self, other: &Self) -> bool {
        // Tests assert `provider.calls[0].tools == [tool]`: compare by structural
        // identity (name + label + description + parameters). Executors are not
        // comparably inspectable, so they are compared by Arc pointer (any two
        // test tools built from the same closure are pointingly distinct, but
        // tests only construct one and compare it to itself via the recorded
        // call list).
        self.name == other.name
            && self.label == other.label
            && self.description == other.description
            && self.parameters == other.parameters
            && Arc::ptr_eq(&self.executor, &other.executor)
    }
}

/// A tool's recorded call for `FakeProvider`-style inspection.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderCall {
    pub model: String,
    pub system: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<AgentTool>,
}
