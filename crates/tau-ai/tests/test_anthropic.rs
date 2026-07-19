//! Integration tests for the Anthropic provider using wiremock.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Map, Value};
use tau_agent::AgentToolResult;
use tau_agent::tool::{AgentTool, ToolError, ToolExecutor};
use tau_types::{AgentMessage, UserMessage};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use tau_ai::anthropic::{AnthropicConfig, AnthropicProvider};
use tau_ai::stream::ProviderEvent;

struct NoopExecutor;

#[async_trait]
impl ToolExecutor for NoopExecutor {
    async fn execute(
        &self,
        _tool_call_id: &str,
        _arguments: &Map<String, Value>,
        _signal: Option<CancellationToken>,
        _on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::from_text("ok"))
    }
}

fn test_config(base_url: &str) -> AnthropicConfig {
    AnthropicConfig {
        api_key: "test-key".to_string(),
        base_url: base_url.to_string(),
        model: "claude-sonnet-4-20250514".to_string(),
        max_retries: 0,
        timeout_seconds: 10,
        ..Default::default()
    }
}

fn test_tool() -> AgentTool {
    AgentTool {
        name: Arc::from("bash"),
        label: "Bash".to_string(),
        description: "Run a bash command".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The command to run"}
            },
            "required": ["command"]
        }),
        executor: Arc::new(NoopExecutor),
        prompt_snippet: None,
        prompt_guidelines: vec![],
        prepare_arguments: None,
        execution_mode: Default::default(),
        render_call: None,
        render_result: None,
    }
}

fn simple_sse_response() -> String {
    [
        r#"data: {"type":"message_start","message":{"id":"msg-1","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#,
        "",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n")
}

fn tool_use_sse_response() -> String {
    [
        r#"data: {"type":"message_start","message":{"id":"msg-2","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#,
        "",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"bash"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"comm"}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"and\":\"ls\"}"}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n")
}

fn thinking_sse_response() -> String {
    [
        r#"data: {"type":"message_start","message":{"id":"msg-3","type":"message","role":"assistant","content":[],"model":"claude-sonnet-4-20250514","stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}}"#,
        "",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "",
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        "",
        r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The answer is 42."}}"#,
        "",
        r#"data: {"type":"content_block_stop","index":1}"#,
        "",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":30}}"#,
        "",
        r#"data: {"type":"message_stop"}"#,
        "",
    ]
    .join("\n")
}

async fn collect_events(stream: impl futures::Stream<Item = ProviderEvent>) -> Vec<ProviderEvent> {
    futures::pin_mut!(stream);
    let mut events = Vec::new();
    while let Some(ev) = stream.next().await {
        events.push(ev);
    }
    events
}

#[tokio::test]
async fn anthropic_simple_text_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(simple_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = AnthropicProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools, None)).await;

    assert!(matches!(events[0], ProviderEvent::ResponseStart { .. }));
    assert!(matches!(&events[1], ProviderEvent::TextDelta(t) if t == "Hello"));
    assert!(matches!(&events[2], ProviderEvent::TextDelta(t) if t == " world"));
    assert!(matches!(&events[3], ProviderEvent::ResponseEnd { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test]
async fn anthropic_tool_use_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tool_use_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = AnthropicProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("List files"))];
    let tools = vec![test_tool()];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools, None)).await;

    assert!(matches!(events[0], ProviderEvent::ResponseStart { .. }));
    if let ProviderEvent::ToolCall(tc) = &events[1] {
        assert_eq!(tc.name, "bash");
        assert_eq!(tc.arguments["command"], "ls");
    } else {
        panic!("expected ToolCall, got {:?}", events[1]);
    }
    if let ProviderEvent::ResponseEnd {
        finish_reason: Some(fr),
        ..
    } = &events[2]
    {
        assert_eq!(fr, "tool_use");
    } else {
        panic!("expected ResponseEnd with tool_use, got {:?}", events[2]);
    }
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn anthropic_thinking_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(thinking_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = AnthropicProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("What is 6*7?"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools, None)).await;

    assert!(matches!(events[0], ProviderEvent::ResponseStart { .. }));
    assert!(matches!(&events[1], ProviderEvent::ThinkingDelta(t) if t == "Let me think..."));
    assert!(matches!(&events[2], ProviderEvent::TextDelta(t) if t == "The answer is 42."));
    assert!(matches!(&events[3], ProviderEvent::ResponseEnd { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test]
async fn anthropic_http_error_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"type":"invalid_request_error","message":"Invalid model"}}"#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = AnthropicProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools, None)).await;

    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], ProviderEvent::Error { message, .. } if message.contains("400")));
}

#[tokio::test]
async fn anthropic_request_body_has_correct_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_string(simple_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = AnthropicProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools = vec![test_tool()];

    let _events =
        collect_events(provider.stream_response("System prompt", &messages, &tools, None)).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "claude-sonnet-4-20250514");
    assert_eq!(body["stream"], true);
    assert_eq!(body["system"], "System prompt");
    assert!(body["tools"].is_array());
    let t = body["tools"].as_array().unwrap();
    assert_eq!(t[0]["name"], "bash");
    assert!(body["messages"].is_array());
}
