//! Anthropic Messages API provider adapter.
//!
//! Translates Anthropic's SSE stream into Pi-compatible `ProviderEvent`s
//! that feed into the shared `canonicalize_provider_stream`.

use std::collections::HashMap;

use async_stream::stream;
use serde_json::{Value, json};
use tau_agent::tool::AgentTool;
use tau_types::{AgentMessage, AssistantMessage, ToolCall, Usage};

use crate::http::{HttpClientConfig, build_client};
use crate::retry::{is_retryable_status, retry_delay_seconds, wait_for_retry};
use crate::stream::ProviderEvent;
use tracing::warn;

const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Configuration for the Anthropic provider.
#[derive(Debug, Clone)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub max_retries: u32,
    pub max_retry_delay_seconds: f64,
    pub timeout_seconds: u64,
    pub headers: Option<Vec<(String, String)>>,
    pub thinking_budget_tokens: Option<u32>,
    pub thinking_mode: Option<String>,
    pub thinking_effort: Option<String>,
    pub oauth_system_prompt: Option<String>,
    pub provider_name: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        AnthropicConfig {
            api_key: String::new(),
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: None,
            max_retries: 5,
            max_retry_delay_seconds: 30.0,
            timeout_seconds: 60,
            headers: None,
            thinking_budget_tokens: None,
            thinking_mode: None,
            thinking_effort: None,
            oauth_system_prompt: None,
            provider_name: "anthropic".to_string(),
        }
    }
}

/// Anthropic Messages API provider.
#[derive(Clone)]
pub struct AnthropicProvider {
    config: AnthropicConfig,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(config: AnthropicConfig) -> Self {
        let client = build_client(&HttpClientConfig {
            timeout: std::time::Duration::from_secs(config.timeout_seconds),
            ..Default::default()
        })
        .expect("failed to build HTTP client");
        AnthropicProvider { config, client }
    }

    /// Stream one response as `ProviderEvent`s.
    pub fn stream_response(
        &self,
        system: &str,
        messages: &[AgentMessage],
        tools: &[AgentTool],
    ) -> impl futures::Stream<Item = ProviderEvent> + Send + 'static {
        let config = self.config.clone();
        let client = self.client.clone();
        let system = system.to_string();
        let messages = messages.to_vec();
        let tools = tools.to_vec();

        stream! {
            let url = format!("{}/v1/messages", config.base_url.trim_end_matches('/'));
            let payload = build_payload(&config, &system, &messages, &tools);

            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert("anthropic-version", ANTHROPIC_VERSION.parse().unwrap());
            headers.insert("content-type", "application/json".parse().unwrap());
            if let Some(ref extra) = config.headers {
                for (k, v) in extra {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                        reqwest::header::HeaderValue::from_str(v),
                    ) {
                        headers.insert(name, val);
                    }
                }
            }
            headers.insert(
                "x-api-key",
                reqwest::header::HeaderValue::from_str(&config.api_key).unwrap(),
            );

            let mut attempt: u32 = 0;
            loop {
                let response = client
                    .post(&url)
                    .headers(headers.clone())
                    .json(&payload)
                    .send()
                    .await;

                match response {
                    Ok(resp) if resp.status().as_u16() >= 400 => {
                        let status = resp.status().as_u16();
                        let body = resp.text().await.unwrap_or_default();
                        if is_retryable_status(status) && attempt < config.max_retries {
                            let delay = retry_delay_seconds(attempt, config.max_retry_delay_seconds);
                            warn!(
                                provider = "anthropic",
                                status,
                                attempt,
                                max = config.max_retries,
                                delay_secs = delay,
                                "retryable HTTP error, retrying"
                            );
                            attempt += 1;
                            if !wait_for_retry(delay, None).await {
                                return;
                            }
                            continue;
                        }
                        yield ProviderEvent::Error {
                            message: format!("HTTP {status}: {body}"),
                            data: None,
                        };
                        return;
                    }
                    Ok(resp) => {
                        let text = resp.text().await.unwrap_or_default();
                        let mut tool_builders: HashMap<u32, ToolCallBuilder> = HashMap::new();
                        let mut finish_reason: Option<String> = None;
                        let mut usage: Usage = Usage::default();

                        for line in text.lines() {
                            let event = match parse_sse_line(line) {
                                Some(e) => e,
                                None => continue,
                            };
                            let chunk: Value = match serde_json::from_str(&event) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let event_type = chunk["type"].as_str().unwrap_or("");

                            match event_type {
                                "message_start" => {
                                    let msg = &chunk["message"];
                                    usage = parse_usage(&msg["usage"]);
                                    let model = msg["model"].as_str().unwrap_or(&config.model);
                                    yield ProviderEvent::ResponseStart {
                                        model: model.to_string(),
                                    };
                                }
                                "content_block_start" => {
                                    let block = &chunk["content_block"];
                                    if block["type"].as_str() == Some("tool_use") {
                                        let index = chunk["index"].as_u64().unwrap_or(0) as u32;
                                        let builder = tool_builders
                                            .entry(index)
                                            .or_default();
                                        builder.id = block["id"].as_str().unwrap_or("").to_string();
                                        builder.name = block["name"].as_str().unwrap_or("").to_string();
                                    }
                                }
                                "content_block_delta" => {
                                    let delta = &chunk["delta"];
                                    match delta["type"].as_str().unwrap_or("") {
                                        "text_delta" => {
                                            let text = delta["text"].as_str().unwrap_or("");
                                            if !text.is_empty() {
                                                yield ProviderEvent::TextDelta(text.to_string());
                                            }
                                        }
                                        "thinking_delta" => {
                                            let text = delta["thinking"].as_str().unwrap_or("");
                                            if !text.is_empty() {
                                                yield ProviderEvent::ThinkingDelta(text.to_string());
                                            }
                                        }
                                        "input_json_delta" => {
                                            let index = chunk["index"].as_u64().unwrap_or(0) as u32;
                                            let builder = tool_builders
                                                .entry(index)
                                                .or_default();
                                            builder.arguments_parts.push(
                                                delta["partial_json"].as_str().unwrap_or("").to_string(),
                                            );
                                        }
                                        _ => {}
                                    }
                                }
                                "message_delta" => {
                                    let delta = &chunk["delta"];
                                    if let Some(sr) = delta["stop_reason"].as_str() {
                                        finish_reason = Some(sr.to_string());
                                    }
                                    let delta_usage = &chunk["usage"];
                                    if let Some(tokens) = delta_usage["output_tokens"].as_i64() {
                                        usage.output = tokens;
                                    }
                                    if let Some(details) =
                                        delta_usage["output_tokens_details"].as_object()
                                    {
                                        if let Some(thinking) =
                                            details.get("thinking_tokens").and_then(|v| v.as_i64())
                                        {
                                            usage.reasoning = Some(thinking);
                                        }
                                    }
                                }
                                "error" => {
                                    let msg = chunk["error"]["message"]
                                        .as_str()
                                        .unwrap_or("Provider returned an error");
                                    yield ProviderEvent::Error {
                                        message: msg.to_string(),
                                        data: Some(chunk),
                                    };
                                    return;
                                }
                                _ => {}
                            }
                        }

                        // Build tool calls
                        let mut tool_calls: Vec<ToolCall> = tool_builders
                            .into_iter()
                            .map(|(idx, b)| b.build(idx))
                            .collect();
                        tool_calls.sort_by(|a, b| a.id.cmp(&b.id));
                        for tc in tool_calls {
                            yield ProviderEvent::ToolCall(tc);
                        }

                        let message = AssistantMessage {
                            content: vec![],
                            usage,
                            ..Default::default()
                        };
                        yield ProviderEvent::ResponseEnd {
                            message,
                            finish_reason,
                        };
                        return;
                    }
                    Err(e) => {
                        if attempt < config.max_retries {
                            let delay = retry_delay_seconds(attempt, config.max_retry_delay_seconds);
                            warn!(
                                provider = "anthropic",
                                error = %e,
                                attempt,
                                max = config.max_retries,
                                delay_secs = delay,
                                "network error, retrying"
                            );
                            attempt += 1;
                            if !wait_for_retry(delay, None).await {
                                return;
                            }
                            continue;
                        }
                        yield ProviderEvent::Error {
                            message: e.to_string(),
                            data: None,
                        };
                        return;
                    }
                }
            }
        }
    }
}

fn parse_sse_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    let payload = line.strip_prefix("data:")?.trim();
    if payload == "[DONE]" {
        return None;
    }
    Some(payload.to_string())
}

fn parse_usage(raw: &Value) -> Usage {
    let input = raw["input_tokens"].as_i64().unwrap_or(0);
    let output = raw["output_tokens"].as_i64().unwrap_or(0);
    let cache_read = raw["cache_read_input_tokens"].as_i64().unwrap_or(0);
    let cache_write = raw["cache_creation_input_tokens"].as_i64().unwrap_or(0);
    let cache_creation = &raw["cache_creation"];
    let cache_write_1h = cache_creation
        .get("ephemeral_1h_input_tokens")
        .and_then(|v| v.as_i64());
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        total_tokens: input + output + cache_read + cache_write,
        cache_write_1h,
        ..Default::default()
    }
}

fn build_payload(
    config: &AnthropicConfig,
    system: &str,
    messages: &[AgentMessage],
    tools: &[AgentTool],
) -> Value {
    let resolved_max_tokens = config.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS);
    let max_tokens = if let Some(budget) = config.thinking_budget_tokens {
        resolved_max_tokens.max(budget + 1024)
    } else {
        resolved_max_tokens
    };

    let mut payload = json!({
        "model": config.model,
        "max_tokens": max_tokens,
        "stream": true,
        "system": if let Some(ref oauth) = config.oauth_system_prompt {
            json!([
                {"type": "text", "text": oauth},
                {"type": "text", "text": system}
            ])
        } else {
            json!(system)
        },
        "messages": messages.iter().map(anthropic_message).collect::<Vec<_>>(),
    });

    if let Some(ref mode) = config.thinking_mode {
        if mode == "adaptive" {
            if let Some(ref effort) = config.thinking_effort {
                payload["thinking"] = json!({"type": "adaptive", "display": "summarized"});
                payload["output_config"] = json!({"effort": effort});
            }
        } else if let Some(budget) = config.thinking_budget_tokens {
            payload["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget,
            });
        }
    } else if let Some(budget) = config.thinking_budget_tokens {
        payload["thinking"] = json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if !tools.is_empty() {
        payload["tools"] = json!(tools.iter().map(anthropic_tool).collect::<Vec<_>>());
    }

    payload
}

fn anthropic_message(msg: &AgentMessage) -> Value {
    match msg {
        AgentMessage::User(um) => json!({
            "role": "user",
            "content": um.text(),
        }),
        AgentMessage::Assistant(am) => {
            let mut content: Vec<Value> = Vec::new();
            for block in &am.content {
                match block {
                    tau_types::AssistantContent::Text(tc) => {
                        content.push(json!({"type": "text", "text": tc.text}));
                    }
                    tau_types::AssistantContent::Thinking(thc) => {
                        let mut obj = json!({
                            "type": "thinking",
                            "thinking": thc.thinking,
                        });
                        if let Some(ref sig) = thc.thinking_signature {
                            obj["signature"] = json!(sig);
                        }
                        content.push(obj);
                    }
                    tau_types::AssistantContent::ToolCall(tc) => {
                        content.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                }
            }
            json!({
                "role": "assistant",
                "content": content,
            })
        }
        AgentMessage::ToolResult(tr) => {
            json!({
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": tr.tool_call_id,
                    "content": tr.text(),
                    "is_error": tr.is_error,
                }],
            })
        }
        _ => json!({
            "role": "user",
            "content": msg.text(),
        }),
    }
}

fn anthropic_tool(tool: &AgentTool) -> Value {
    json!({
        "name": tool.name(),
        "description": tool.description,
        "input_schema": tool.input_schema(),
    })
}

/// Accumulator for tool call JSON fragments (Anthropic sends them incrementally).
#[derive(Default)]
struct ToolCallBuilder {
    id: String,
    name: String,
    arguments_parts: Vec<String>,
}

impl ToolCallBuilder {
    fn build(self, index: u32) -> ToolCall {
        let raw = self.arguments_parts.join("");
        let arguments = if raw.is_empty() {
            serde_json::Map::new()
        } else {
            serde_json::from_str::<serde_json::Map<String, Value>>(&raw).unwrap_or_else(|_| {
                let mut m = serde_json::Map::new();
                m.insert("_raw_arguments".to_string(), Value::String(raw));
                m
            })
        };
        let id = if self.id.is_empty() {
            format!("tool-call-{index}")
        } else {
            self.id
        };
        ToolCall::new(id, self.name).with_arguments(arguments)
    }
}

trait ToolCallExt {
    fn with_arguments(self, arguments: serde_json::Map<String, Value>) -> ToolCall;
}

impl ToolCallExt for ToolCall {
    fn with_arguments(mut self, arguments: serde_json::Map<String, Value>) -> ToolCall {
        self.arguments = arguments;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sse_lines() {
        assert!(parse_sse_line("").is_none());
        assert!(parse_sse_line(": comment").is_none());
        assert!(parse_sse_line("data: [DONE]").is_none());
        let data = parse_sse_line(r#"data: {"type":"delta"}"#).unwrap();
        assert!(data.contains("delta"));
    }

    #[test]
    fn tool_builder_merges_arguments() {
        let mut b = ToolCallBuilder {
            id: "c1".into(),
            name: "bash".into(),
            ..Default::default()
        };
        b.arguments_parts.push(r#"{"command":"#.into());
        b.arguments_parts.push(r#""ls"}"#.into());
        let tc = b.build(0);
        assert_eq!(tc.id, "c1");
        assert_eq!(tc.name, "bash");
        assert_eq!(tc.arguments["command"], "ls");
    }

    #[test]
    fn tool_builder_fallback_id() {
        let b = ToolCallBuilder {
            id: String::new(),
            name: "bash".into(),
            arguments_parts: vec![],
        };
        let tc = b.build(5);
        assert_eq!(tc.id, "tool-call-5");
    }

    #[test]
    fn anthropic_message_user() {
        let msg = AgentMessage::User(tau_types::UserMessage::new("hello"));
        let v = anthropic_message(&msg);
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"], "hello");
    }
}

// ---------------------------------------------------------------------------
// ModelProvider implementation
// ---------------------------------------------------------------------------

use crate::stream::canonicalize_provider_stream;
use futures::stream::BoxStream;
use tau_agent::provider::{ModelProvider, StreamRequest};
use tau_types::AssistantMessageEvent;

/// Wrapper that implements ModelProvider for AnthropicProvider.
#[derive(Clone)]
pub struct AnthropicModelProvider {
    inner: AnthropicProvider,
}

impl AnthropicModelProvider {
    pub fn new(provider: AnthropicProvider) -> Self {
        Self { inner: provider }
    }
}

impl ModelProvider for AnthropicModelProvider {
    fn stream_response<'a>(
        &'a self,
        request: &'a StreamRequest<'a>,
    ) -> BoxStream<'a, AssistantMessageEvent> {
        let provider_stream =
            self.inner
                .stream_response(request.system, request.messages, request.tools);

        Box::pin(canonicalize_provider_stream(provider_stream))
    }
}
