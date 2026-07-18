//! Integration tests for the OpenAI provider using wiremock.

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

use tau_ai::openai::{OpenAIConfig, OpenAIProvider};
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

fn test_config(base_url: &str) -> OpenAIConfig {
    OpenAIConfig {
        api_key: "test-key".to_string(),
        base_url: base_url.to_string(),
        model: "gpt-4o".to_string(),
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
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n")
}

fn tool_use_sse_response() -> String {
    [
        r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call-1","type":"function","function":{"name":"bash","arguments":""}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"co"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"mmand"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\":\"ls\"}"}}]},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-2","object":"chat.completion.chunk","created":1700000000,"model":"gpt-4o","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
        "",
        "data: [DONE]",
        "",
    ]
    .join("\n")
}

fn reasoning_sse_response() -> String {
    [
        r#"data: {"id":"chatcmpl-3","object":"chat.completion.chunk","created":1700000000,"model":"o1","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"Let me think..."},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-3","object":"chat.completion.chunk","created":1700000000,"model":"o1","choices":[{"index":0,"delta":{"content":"The answer is 42."},"finish_reason":null}]}"#,
        "",
        r#"data: {"id":"chatcmpl-3","object":"chat.completion.chunk","created":1700000000,"model":"o1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        "",
        "data: [DONE]",
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
async fn openai_simple_text_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(simple_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = OpenAIProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools)).await;

    assert!(matches!(events[0], ProviderEvent::ResponseStart { .. }));
    assert!(matches!(&events[1], ProviderEvent::TextDelta(t) if t == "Hello"));
    assert!(matches!(&events[2], ProviderEvent::TextDelta(t) if t == " world"));
    assert!(matches!(&events[3], ProviderEvent::ResponseEnd { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test]
async fn openai_tool_use_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tool_use_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = OpenAIProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("List files"))];
    let tools = vec![test_tool()];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools)).await;

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
        assert_eq!(fr, "tool_calls");
    } else {
        panic!("expected ResponseEnd with tool_calls, got {:?}", events[2]);
    }
    assert_eq!(events.len(), 3);
}

#[tokio::test]
async fn openai_reasoning_stream() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(reasoning_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = OpenAIProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("What is 6*7?"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools)).await;

    assert!(matches!(events[0], ProviderEvent::ResponseStart { .. }));
    assert!(matches!(&events[1], ProviderEvent::ThinkingDelta(t) if t == "Let me think..."));
    assert!(matches!(&events[2], ProviderEvent::TextDelta(t) if t == "The answer is 42."));
    assert!(matches!(&events[3], ProviderEvent::ResponseEnd { .. }));
    assert_eq!(events.len(), 4);
}

#[tokio::test]
async fn openai_http_error_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"error":{"message":"Unauthorized"}}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = OpenAIProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools: Vec<AgentTool> = vec![];

    let events =
        collect_events(provider.stream_response("You are helpful.", &messages, &tools)).await;

    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], ProviderEvent::Error { message, .. } if message.contains("401")));
}

#[tokio::test]
async fn openai_request_body_has_correct_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(simple_sse_response()))
        .expect(1)
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let provider = OpenAIProvider::new(config);
    let messages = vec![AgentMessage::User(UserMessage::new("Hi"))];
    let tools = vec![test_tool()];

    let _events =
        collect_events(provider.stream_response("System prompt", &messages, &tools)).await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);

    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["stream"], true);
    assert!(body["messages"].is_array());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[0]["content"], "System prompt");
    assert_eq!(msgs[1]["role"], "user");
    assert_eq!(msgs[1]["content"], "Hi");
    assert!(body["tools"].is_array());
    let t = body["tools"].as_array().unwrap();
    assert_eq!(t[0]["type"], "function");
    assert_eq!(t[0]["function"]["name"], "bash");
}
