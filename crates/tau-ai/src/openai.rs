//! OpenAI-compatible chat completions provider adapter.
//!
//! Translates an OpenAI SSE stream into Pi-compatible `ProviderEvent`s.
//! Works with any provider that implements the `/chat/completions` endpoint
//! (OpenAI, Azure, vLLM, Ollama, etc.).

use std::collections::HashMap;

use async_stream::stream;
use serde_json::{Value, json};
use tau_agent::tool::AgentTool;
use tau_types::{AgentMessage, AssistantMessage, ToolCall};

use crate::http::{HttpClientConfig, build_client};
use crate::retry::{
    is_retryable_status, rate_limit_delay_seconds, retry_after_seconds, retry_delay_seconds,
    wait_for_retry,
};
use crate::stream::ProviderEvent;
use tracing::warn;

const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Configuration for an OpenAI-compatible provider.
#[derive(Debug, Clone)]
pub struct OpenAIConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub max_tokens: Option<u32>,
    pub max_retries: u32,
    pub max_retry_delay_seconds: f64,
    pub timeout_seconds: u64,
    pub headers: Option<Vec<(String, String)>>,
    pub provider_name: String,
}

impl Default for OpenAIConfig {
    fn default() -> Self {
        OpenAIConfig {
            api_key: String::new(),
            base_url: "https://api.openai.com".to_string(),
            model: "gpt-4o".to_string(),
            max_tokens: None,
            max_retries: 5,
            max_retry_delay_seconds: 30.0,
            timeout_seconds: 60,
            headers: None,
            provider_name: "openai".to_string(),
        }
    }
}

/// OpenAI-compatible chat completions provider.
#[derive(Clone)]
pub struct OpenAIProvider {
    config: OpenAIConfig,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(config: OpenAIConfig) -> Self {
        let client = build_client(&HttpClientConfig {
            timeout: std::time::Duration::from_secs(config.timeout_seconds),
            ..Default::default()
        })
        .expect("failed to build HTTP client");
        OpenAIProvider { config, client }
    }

    /// Stream one response as `ProviderEvent`s.
    pub fn stream_response(
        &self,
        system: &str,
        messages: &[AgentMessage],
        tools: &[AgentTool],
        thinking_level: Option<&str>,
    ) -> impl futures::Stream<Item = ProviderEvent> + Send + 'static {
        let config = self.config.clone();
        let client = self.client.clone();
        let system = system.to_string();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let thinking_level = thinking_level.map(|s| s.to_string());

        stream! {
            let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
            let payload = build_payload(&config, &system, &messages, &tools, thinking_level.as_deref());

            let mut headers = reqwest::header::HeaderMap::new();
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
                "authorization",
                reqwest::header::HeaderValue::from_str(&format!("Bearer {}", config.api_key)).unwrap(),
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
                        let retry_after = retry_after_seconds(resp.headers());
                        let body = resp.text().await.unwrap_or_default();
                        if is_retryable_status(status) && attempt < config.max_retries {
                            // Rate-limit (429) gets a longer, Retry-After-aware
                            // backoff so free tiers don't burn attempts instantly.
                            let delay = if status == 429 {
                                rate_limit_delay_seconds(
                                    attempt,
                                    retry_after,
                                    config.max_retry_delay_seconds,
                                )
                            } else {
                                retry_delay_seconds(attempt, config.max_retry_delay_seconds)
                            };
                            warn!(
                                provider = config.provider_name,
                                status,
                                attempt,
                                max = config.max_retries,
                                delay_secs = delay,
                                retry_after = ?retry_after,
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
                        use futures::StreamExt;
                        let status = resp.status().as_u16();
                        let content_length = resp
                            .headers()
                            .get("content-length")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("unknown")
                            .to_string();
                        let mut byte_stream = resp.bytes_stream();
                        let mut line_buf = String::new();

                        // Read first chunk — if empty, retry (matches old behavior)
                        let first_chunk = byte_stream.next().await;
                        let first_bytes = match first_chunk {
                            Some(Ok(b)) => b,
                            Some(Err(e)) => {
                                if attempt < config.max_retries {
                                    let delay = retry_delay_seconds(attempt, config.max_retry_delay_seconds);
                                    warn!(
                                        provider = config.provider_name,
                                        error = %e,
                                        attempt,
                                        max = config.max_retries,
                                        delay_secs = delay,
                                        "network error reading SSE body, retrying"
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
                            None => {
                                if attempt < config.max_retries {
                                    let delay =
                                        retry_delay_seconds(attempt, config.max_retry_delay_seconds);
                                    warn!(
                                        provider = config.provider_name,
                                        status,
                                        content_length,
                                        attempt,
                                        max = config.max_retries,
                                        delay_secs = delay,
                                        "empty response body, retrying"
                                    );
                                    attempt += 1;
                                    if !wait_for_retry(delay, None).await {
                                        return;
                                    }
                                    continue;
                                }
                                warn!(
                                    provider = config.provider_name,
                                    status,
                                    content_length,
                                    "empty response body after all retries"
                                );
                                yield ProviderEvent::Error {
                                    message: format!("HTTP {status}: empty response body"),
                                    data: None,
                                };
                                return;
                            }
                        };
                        line_buf.push_str(&String::from_utf8_lossy(&first_bytes));

                        let mut tool_builders: HashMap<u32, ToolCallBuilder> = HashMap::new();
                        let mut finish_reason: Option<String> = None;
                        let mut started = false;
                        let mut resolved_model: String = config.model.clone();
                        let mut lines_processed: u32 = 0;
                        let mut content_chunks: u32 = 0;
                        let mut sse_error: Option<String> = None;

                        // Process the first chunk's complete lines
                        while let Some(pos) = line_buf.find('\n') {
                            let line = line_buf[..pos].to_string();
                            line_buf = line_buf[pos + 1..].to_string();
                            let line = line.trim();
                            let payload = match line.strip_prefix("data:") {
                                Some(rest) => rest.trim(),
                                None => continue,
                            };
                            if payload == "[DONE]" {
                                break;
                            }
                            let chunk: Value = match serde_json::from_str(payload) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            lines_processed += 1;

                            if let Some(err) = chunk.get("error") {
                                let msg = err["message"]
                                    .as_str()
                                    .unwrap_or("unknown provider error")
                                    .to_string();
                                sse_error = Some(msg);
                                break;
                            }

                            let choices = match chunk["choices"].as_array() {
                                Some(c) if !c.is_empty() => &c[0],
                                _ => continue,
                            };
                            let delta = &choices["delta"];

                            if !started {
                                started = true;
                                let model = chunk["model"].as_str().unwrap_or(&config.model);
                                resolved_model = model.to_string();
                                yield ProviderEvent::ResponseStart {
                                    model: model.to_string(),
                                };
                            }

                            if let Some(text_content) = delta["content"].as_str() {
                                if !text_content.is_empty() {
                                    content_chunks += 1;
                                    yield ProviderEvent::TextDelta(text_content.to_string());
                                }
                            }
                            if let Some(reasoning) = delta["reasoning_content"].as_str() {
                                if !reasoning.is_empty() {
                                    yield ProviderEvent::ThinkingDelta(reasoning.to_string());
                                }
                            }
                            if let Some(tc_deltas) = delta["tool_calls"].as_array() {
                                for tc_delta in tc_deltas {
                                    let index = tc_delta["index"].as_u64().unwrap_or(0) as u32;
                                    let builder = tool_builders.entry(index).or_default();
                                    if let Some(id) = tc_delta["id"].as_str() {
                                        builder.id = id.to_string();
                                    }
                                    if let Some(name) = tc_delta["function"]["name"].as_str() {
                                        builder.name = name.to_string();
                                    }
                                    if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                                        builder.arguments_parts.push(args.to_string());
                                    }
                                }
                            }
                            if let Some(fr) = choices["finish_reason"].as_str() {
                                if !fr.is_empty() {
                                    finish_reason = Some(fr.to_string());
                                }
                            }
                        }

                        // Process remaining chunks
                        while let Some(chunk_result) = byte_stream.next().await {
                            let bytes = match chunk_result {
                                Ok(b) => b,
                                Err(e) => {
                                    warn!(
                                        provider = config.provider_name,
                                        error = %e,
                                        "error reading SSE body chunk"
                                    );
                                    break;
                                }
                            };
                            line_buf.push_str(&String::from_utf8_lossy(&bytes));

                            while let Some(pos) = line_buf.find('\n') {
                                let line = line_buf[..pos].to_string();
                                line_buf = line_buf[pos + 1..].to_string();
                                let line = line.trim();
                                let payload = match line.strip_prefix("data:") {
                                    Some(rest) => rest.trim(),
                                    None => continue,
                                };
                                if payload == "[DONE]" {
                                    break;
                                }
                                let chunk: Value = match serde_json::from_str(payload) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                                lines_processed += 1;

                                if let Some(err) = chunk.get("error") {
                                    let msg = err["message"]
                                        .as_str()
                                        .unwrap_or("unknown provider error")
                                        .to_string();
                                    sse_error = Some(msg);
                                    break;
                                }

                                let choices = match chunk["choices"].as_array() {
                                    Some(c) if !c.is_empty() => &c[0],
                                    _ => continue,
                                };
                                let delta = &choices["delta"];

                                if !started {
                                    started = true;
                                    let model = chunk["model"].as_str().unwrap_or(&config.model);
                                    resolved_model = model.to_string();
                                    yield ProviderEvent::ResponseStart {
                                        model: model.to_string(),
                                    };
                                }

                                if let Some(text_content) = delta["content"].as_str() {
                                    if !text_content.is_empty() {
                                        content_chunks += 1;
                                        yield ProviderEvent::TextDelta(text_content.to_string());
                                    }
                                }
                                if let Some(reasoning) = delta["reasoning_content"].as_str() {
                                    if !reasoning.is_empty() {
                                        yield ProviderEvent::ThinkingDelta(reasoning.to_string());
                                    }
                                }
                                if let Some(tc_deltas) = delta["tool_calls"].as_array() {
                                    for tc_delta in tc_deltas {
                                        let index = tc_delta["index"].as_u64().unwrap_or(0) as u32;
                                        let builder = tool_builders.entry(index).or_default();
                                        if let Some(id) = tc_delta["id"].as_str() {
                                            builder.id = id.to_string();
                                        }
                                        if let Some(name) = tc_delta["function"]["name"].as_str() {
                                            builder.name = name.to_string();
                                        }
                                        if let Some(args) = tc_delta["function"]["arguments"].as_str() {
                                            builder.arguments_parts.push(args.to_string());
                                        }
                                    }
                                }
                                if let Some(fr) = choices["finish_reason"].as_str() {
                                    if !fr.is_empty() {
                                        finish_reason = Some(fr.to_string());
                                    }
                                }
                            }

                            if sse_error.is_some() {
                                break;
                            }
                        }

                        // Handle SSE-wrapped errors with retry
                        if let Some(msg) = sse_error {
                            if attempt < config.max_retries {
                                let delay =
                                    retry_delay_seconds(attempt, config.max_retry_delay_seconds);
                                warn!(
                                    provider = config.provider_name,
                                    error = %msg,
                                    attempt,
                                    max = config.max_retries,
                                    delay_secs = delay,
                                    "SSE-wrapped error, retrying"
                                );
                                attempt += 1;
                                if !wait_for_retry(delay, None).await {
                                    return;
                                }
                                continue;
                            }
                            yield ProviderEvent::Error {
                                message: msg,
                                data: None,
                            };
                            return;
                        }

                        // Emit tool calls
                        let mut tool_calls: Vec<ToolCall> = tool_builders
                            .into_iter()
                            .map(|(idx, b)| b.build(idx))
                            .collect();
                        tool_calls.sort_by(|a, b| a.id.cmp(&b.id));

                        if content_chunks == 0 && tool_calls.is_empty() {
                            warn!(
                                provider = config.provider_name,
                                lines_processed,
                                started,
                                ?finish_reason,
                                "no text content or tool calls in response"
                            );
                        }

                        for tc in tool_calls {
                            yield ProviderEvent::ToolCall(tc);
                        }

                        if !started {
                            yield ProviderEvent::ResponseStart {
                                model: config.model,
                            };
                        }

                        let message = AssistantMessage {
                            model: resolved_model.clone(),
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
                                provider = config.provider_name,
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

fn build_payload(
    config: &OpenAIConfig,
    system: &str,
    messages: &[AgentMessage],
    tools: &[AgentTool],
    thinking_level: Option<&str>,
) -> Value {
    let mut oai_messages: Vec<Value> = Vec::new();

    // System message
    if !system.is_empty() {
        oai_messages.push(json!({
            "role": "system",
            "content": system,
        }));
    }

    for msg in messages {
        match msg {
            AgentMessage::User(um) => {
                oai_messages.push(json!({
                    "role": "user",
                    "content": um.text(),
                }));
            }
            AgentMessage::Assistant(am) => {
                let mut content = String::new();
                for block in &am.content {
                    match block {
                        tau_types::AssistantContent::Text(tc) => {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(&tc.text);
                        }
                        tau_types::AssistantContent::Thinking(_) => {}
                        tau_types::AssistantContent::ToolCall(tc) => {
                            // Tool calls are sent as assistant message with tool_calls
                            let mut msg = json!({"role": "assistant"});
                            if !content.is_empty() {
                                msg["content"] = json!(content);
                            }
                            msg["tool_calls"] = json!([{
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.name,
                                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                                }
                            }]);
                            oai_messages.push(msg);
                            content = String::new();
                        }
                    }
                }
                if !content.is_empty() {
                    oai_messages.push(json!({
                        "role": "assistant",
                        "content": content,
                    }));
                }
            }
            AgentMessage::ToolResult(tr) => {
                oai_messages.push(json!({
                    "role": "tool",
                    "tool_call_id": tr.tool_call_id,
                    "content": tr.text(),
                }));
            }
            _ => {
                oai_messages.push(json!({
                    "role": "user",
                    "content": msg.text(),
                }));
            }
        }
    }

    let mut payload = json!({
        "model": config.model,
        "messages": oai_messages,
        "stream": true,
    });

    if let Some(max) = config.max_tokens {
        payload["max_tokens"] = json!(max);
    } else {
        payload["max_tokens"] = json!(DEFAULT_MAX_TOKENS);
    }

    if !tools.is_empty() {
        payload["tools"] = json!(tools.iter().map(openai_tool).collect::<Vec<_>>());
    }

    // Thinking/reasoning-effort: OpenAI-compatible providers accept
    // `reasoning_effort`. An empty/`off` level means "use provider default",
    // so we omit the field (mirrors the original `ThinkingLevel.OFF` meaning).
    if let Some(level) = thinking_level {
        if !level.is_empty() && level != "off" {
            payload["reasoning_effort"] = json!(level);
        }
    }

    payload
}

fn openai_tool(tool: &AgentTool) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name(),
            "description": tool.description,
            "parameters": tool.input_schema(),
        }
    })
}

/// Accumulator for OpenAI tool call JSON fragments.
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
    fn tool_builder_merges_args() {
        let mut b = ToolCallBuilder {
            id: "c1".into(),
            name: "read".into(),
            ..Default::default()
        };
        b.arguments_parts.push(r#"{"file_p"#.into());
        b.arguments_parts.push(r#"ath":"/tmp/x"}"#.into());
        let tc = b.build(0);
        assert_eq!(tc.arguments["file_path"], "/tmp/x");
    }

    #[test]
    fn build_payload_includes_reasoning_effort_for_thinking_level() {
        let config = OpenAIConfig::default();
        let messages = vec![tau_types::AgentMessage::User(tau_types::UserMessage::new(
            "Hi",
        ))];
        let tools: Vec<tau_agent::tool::AgentTool> = vec![];

        let with_level = build_payload(&config, "sys", &messages, &tools, Some("high"));
        assert_eq!(
            with_level.get("reasoning_effort").and_then(|v| v.as_str()),
            Some("high")
        );

        // `off` and `None` must not emit the parameter (provider default).
        let off = build_payload(&config, "sys", &messages, &tools, Some("off"));
        assert!(off.get("reasoning_effort").is_none());
        let none = build_payload(&config, "sys", &messages, &tools, None);
        assert!(none.get("reasoning_effort").is_none());
    }
}

// ---------------------------------------------------------------------------
// ModelProvider implementation
// ---------------------------------------------------------------------------

use crate::stream::canonicalize_provider_stream;
use futures::stream::BoxStream;
use tau_agent::provider::{ModelProvider, StreamRequest};
use tau_types::AssistantMessageEvent;

/// Wrapper that implements ModelProvider for OpenAIProvider.
#[derive(Clone)]
pub struct OpenAIModelProvider {
    inner: OpenAIProvider,
}

impl OpenAIModelProvider {
    pub fn new(provider: OpenAIProvider) -> Self {
        Self { inner: provider }
    }
}

impl ModelProvider for OpenAIModelProvider {
    fn stream_response<'a>(
        &'a self,
        request: &'a StreamRequest<'a>,
    ) -> BoxStream<'a, AssistantMessageEvent> {
        let provider_stream = self.inner.stream_response(
            request.system,
            request.messages,
            request.tools,
            request.thinking_level,
        );

        Box::pin(canonicalize_provider_stream(provider_stream))
    }
}
