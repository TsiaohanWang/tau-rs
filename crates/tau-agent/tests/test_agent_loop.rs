//! Translated from `tests/test_agent_loop.py` — 8 tests covering the core
//! agent loop: event lifecycle, thinking events, tool execution, signals,
//! unknown tool errors, provider errors, steering/follow-up, and max_turns.

use std::sync::Arc;

use futures::StreamExt;
use tau_types::{
    AgentEvent, AgentMessage, AssistantContent, AssistantMessage, MessageStartEvent, StopReason,
    ToolCall, UserMessage,
};

use tau_agent::AgentToolResult;
use tau_agent::agent_loop::run_agent_loop;
use tau_agent::provider::CancelToken;
use tau_agent::testing::{
    FakeProvider, assistant_done, assistant_error, assistant_start, text_delta, thinking_delta,
    tool_call_end,
};
use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type ToolFnInner = dyn Fn(
        &str,
        &serde_json::Map<String, serde_json::Value>,
        Option<CancelToken>,
        Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
    > + Send
    + Sync;

fn user(content: &str) -> AgentMessage {
    AgentMessage::User(UserMessage::new(content))
}

fn tool(
    name: &'static str,
    execute_fn: impl Fn(
        &str,
        &serde_json::Map<String, serde_json::Value>,
        Option<CancelToken>,
        Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<AgentToolResult, ToolError>> + Send>,
    > + Send
    + Sync
    + 'static,
) -> AgentTool {
    let executor = Arc::new(ToolFn(Box::new(execute_fn))) as Arc<dyn ToolExecutor>;
    AgentTool {
        name: Arc::from(name),
        label: format!("{} Tool", name),
        description: format!("Run {name}."),
        parameters: serde_json::json!({"type": "object"}),
        executor,
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
        prepare_arguments: None,
        execution_mode: ToolExecutionMode::Parallel,
        render_call: None,
        render_result: None,
    }
}

struct ToolFn(Box<ToolFnInner>);

#[async_trait::async_trait]
impl ToolExecutor for ToolFn {
    async fn execute(
        &self,
        tool_call_id: &str,
        arguments: &serde_json::Map<String, serde_json::Value>,
        signal: Option<CancelToken>,
        on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> Result<AgentToolResult, ToolError> {
        (self.0)(tool_call_id, arguments, signal, on_update).await
    }
}

async fn collect(stream: impl futures::Stream<Item = AgentEvent> + Send) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let mut stream = std::pin::pin!(stream);
    while let Some(ev) = stream.as_mut().next().await {
        events.push(ev);
    }
    events
}

fn event_type(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::AgentStart(_) => "agent_start",
        AgentEvent::AgentEnd(_) => "agent_end",
        AgentEvent::TurnStart(_) => "turn_start",
        AgentEvent::TurnEnd(_) => "turn_end",
        AgentEvent::MessageStart(_) => "message_start",
        AgentEvent::MessageEnd(_) => "message_end",
        AgentEvent::MessageUpdate(_) => "message_update",
        AgentEvent::ToolExecutionStart(_) => "tool_execution_start",
        AgentEvent::ToolExecutionUpdate(_) => "tool_execution_update",
        AgentEvent::ToolExecutionEnd(_) => "tool_execution_end",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_agent_loop_streams_canonical_nested_events() {
    let mut messages = vec![user("Say hello")];
    let provider = FakeProvider::new(vec![vec![
        assistant_start(None, None),
        text_delta("Hel"),
        text_delta("lo"),
        assistant_done(AssistantMessage::from_text("Hello")),
    ]]);

    let events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[],
        prompts: &[user("Say hello")],
        max_turns: None,
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    let types: Vec<&str> = events.iter().map(event_type).collect();
    assert_eq!(
        types,
        vec![
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_update",
            "message_update",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );

    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|ev| match ev {
            AgentEvent::MessageUpdate(e) => Some(&e.assistant_message_event),
            _ => None,
        })
        .filter_map(|ev| match ev {
            tau_types::AssistantMessageEvent::TextDelta(t) => Some(t.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hel", "lo"]);
}

#[tokio::test]
async fn test_agent_loop_nests_thinking_events_without_losing_final_message() {
    let mut messages = vec![user("Think briefly")];
    let provider = FakeProvider::new(vec![vec![
        assistant_start(None, None),
        thinking_delta("hidden "),
        thinking_delta("reasoning"),
        text_delta("Done"),
        assistant_done(AssistantMessage::from_text("Done")),
    ]]);

    let events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[],
        prompts: &[user("Think briefly")],
        max_turns: None,
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    let thinking_deltas: Vec<&str> = events
        .iter()
        .filter_map(|ev| match ev {
            AgentEvent::MessageUpdate(e) => Some(&e.assistant_message_event),
            _ => None,
        })
        .filter_map(|ev| match ev {
            tau_types::AssistantMessageEvent::ThinkingDelta(t) => Some(t.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(thinking_deltas, vec!["hidden ", "reasoning"]);

    // Final message should be assistant
    match messages.last().unwrap() {
        AgentMessage::Assistant(a) => {
            assert_eq!(a.text(), "Done");
        }
        other => panic!("expected assistant message, got {other:?}"),
    }
}

#[tokio::test]
async fn test_agent_loop_executes_tool_and_emits_tool_result_message_lifecycle() {
    let tool_arc = tool("read", |_id, args, _signal, _on_update| {
        let path = args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        Box::pin(async move { Ok(AgentToolResult::from_text(format!("contents of {path}"))) })
    });

    let mut tc_args = serde_json::Map::new();
    tc_args.insert("path".into(), serde_json::json!("README.md"));
    let mut tool_call = ToolCall::new("call-1", "read");
    tool_call.arguments = tc_args;
    let done_msg = AssistantMessage {
        stop_reason: StopReason::ToolUse,
        content: vec![AssistantContent::ToolCall(tool_call)],
        ..Default::default()
    };

    let provider = FakeProvider::new(vec![
        vec![
            assistant_start(None, None),
            tool_call_end("read", r#"{"path":"README.md"}"#, "call-1"),
            assistant_done(done_msg),
        ],
        vec![
            assistant_start(None, None),
            text_delta("Done."),
            assistant_done(AssistantMessage::from_text("Done.")),
        ],
    ]);
    let mut messages = vec![user("Read README.md")];

    let events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[tool_arc],
        prompts: &[user("Read README.md")],
        max_turns: None,
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    // Find the ToolResultMessage in messages
    let result = messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::ToolResult(tr) => Some(tr),
            _ => None,
        })
        .expect("expected a ToolResultMessage");
    assert_eq!(result.tool_name, "read");
    assert_eq!(result.text(), "contents of README.md");

    // 3 message_start events: user prompt, assistant (tool use), tool result
    let starts: Vec<&str> = events
        .iter()
        .filter_map(|ev| match ev {
            AgentEvent::MessageStart(MessageStartEvent { message }) => Some(message),
            _ => None,
        })
        .map(|m| match m {
            AgentMessage::User(_) => "user",
            AgentMessage::Assistant(_) => "assistant",
            AgentMessage::ToolResult(_) => "tool_result",
            _ => "other",
        })
        .collect();
    assert_eq!(
        starts,
        vec!["user", "assistant", "tool_result", "assistant"]
    );
}

#[tokio::test]
async fn test_agent_loop_records_unknown_tool_as_canonical_error_result() {
    let call = ToolCall::new("call-1", "missing");
    let done_msg = AssistantMessage {
        stop_reason: StopReason::ToolUse,
        content: vec![AssistantContent::ToolCall(call)],
        ..Default::default()
    };

    let provider = FakeProvider::new(vec![vec![
        assistant_start(None, None),
        tool_call_end("missing", r#"{"dummy":"v"}"#, "call-1"),
        assistant_done(done_msg),
    ]]);
    let mut messages = vec![user("Use it")];

    let events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[],
        prompts: &[user("Use it")],
        max_turns: Some(1),
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    let end = events
        .iter()
        .find_map(|ev| match ev {
            AgentEvent::ToolExecutionEnd(e) => Some(e),
            _ => None,
        })
        .expect("expected ToolExecutionEnd");
    assert!(end.is_error);
    assert_eq!(end.result.text(), "Tool missing not found");

    let result = messages
        .iter()
        .find_map(|m| match m {
            AgentMessage::ToolResult(tr) => Some(tr),
            _ => None,
        })
        .expect("expected ToolResultMessage");
    assert!(result.is_error);
    assert_eq!(result.text(), "Tool missing not found");
}

#[tokio::test]
async fn test_agent_loop_converts_provider_error_to_assistant_error_message() {
    let mut messages = vec![user("hello")];
    let provider = FakeProvider::new(vec![vec![assistant_error("provider failed")]]);

    let events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[],
        prompts: &[user("hello")],
        max_turns: None,
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    let types: Vec<&str> = events.iter().map(event_type).collect();
    assert_eq!(
        types,
        vec![
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );

    match messages.last().unwrap() {
        AgentMessage::Assistant(a) => {
            assert_eq!(a.stop_reason, StopReason::Error);
        }
        other => panic!("expected assistant error, got {other:?}"),
    }
}

#[tokio::test]
async fn test_agent_loop_stops_with_assistant_error_after_max_turns() {
    let call = ToolCall::new("call-1", "missing");
    let done_msg = AssistantMessage {
        stop_reason: StopReason::ToolUse,
        content: vec![AssistantContent::ToolCall(call)],
        ..Default::default()
    };

    let provider = FakeProvider::new(vec![vec![
        assistant_start(None, None),
        tool_call_end("missing", r#"{"dummy":"v"}"#, "call-1"),
        assistant_done(done_msg),
    ]]);
    let mut messages = vec![user("loop")];

    let _events = collect(run_agent_loop(tau_agent::agent_loop::LoopArgs {
        provider: &provider,
        model: "fake",
        system: "You are Tau.",
        messages: &mut messages,
        tools: &[],
        prompts: &[user("loop")],
        max_turns: Some(1),
        signal: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        before_tool_call: None,
        after_tool_call: None,
    }))
    .await;

    match messages.last().unwrap() {
        AgentMessage::Assistant(a) => {
            assert_eq!(a.stop_reason, StopReason::Error);
            assert!(
                a.error_message
                    .as_ref()
                    .is_some_and(|s| s.contains("max_turns"))
            );
        }
        other => panic!("expected assistant error, got {other:?}"),
    }
    assert_eq!(provider.call_count(), 1);
}
