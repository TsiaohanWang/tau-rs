use tau_agent::tool::AgentTool;
use tau_types::{AgentMessage, AssistantContent};

pub const CHARS_PER_TOKEN: usize = 4;
pub const DEFAULT_RESERVE: u64 = 16384;

#[derive(Debug, Clone)]
pub struct ContextUsageEstimate {
    pub estimated_tokens: u64,
    pub message_count: usize,
}

fn message_chars(msg: &AgentMessage) -> usize {
    let text = msg.text();
    let mut chars = text.len();

    match msg {
        AgentMessage::Assistant(a) => {
            for block in &a.content {
                match block {
                    AssistantContent::ToolCall(tc) => {
                        chars += tc.name.len();
                        chars += serde_json::to_string(&tc.arguments)
                            .map(|s| s.len())
                            .unwrap_or(0);
                    }
                    AssistantContent::Thinking(t) => {
                        chars += t.thinking.len();
                    }
                    AssistantContent::Text(_) => {}
                }
            }
        }
        AgentMessage::BashExecution(b) => {
            chars += b.command.len();
        }
        _ => {}
    }

    chars
}

pub fn estimate_context_usage(
    messages: &[AgentMessage],
    tools: &[AgentTool],
) -> ContextUsageEstimate {
    let mut total_chars: usize = messages.iter().map(message_chars).sum();

    for tool in tools {
        total_chars += serde_json::to_string(&tool.parameters)
            .map(|s| s.len())
            .unwrap_or(0);
    }

    let estimated_tokens = (total_chars / CHARS_PER_TOKEN) as u64;
    ContextUsageEstimate {
        estimated_tokens,
        message_count: messages.len(),
    }
}

pub fn needs_compaction(
    estimate: &ContextUsageEstimate,
    context_window: u64,
    reserve: u64,
) -> bool {
    estimate.estimated_tokens >= context_window.saturating_sub(reserve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tau_agent::tool::{ToolError, ToolExecutionMode, ToolExecutor};
    use tau_types::{AssistantMessage, MessageRole, ToolCall, UserMessage};

    struct DummyExecutor;

    #[async_trait::async_trait]
    impl ToolExecutor for DummyExecutor {
        async fn execute(
            &self,
            _: &str,
            _: &serde_json::Map<String, serde_json::Value>,
            _: Option<tokio_util::sync::CancellationToken>,
            _: Option<&(dyn Fn(tau_types::AgentToolResult) + Send + Sync)>,
        ) -> Result<tau_types::AgentToolResult, ToolError> {
            unreachable!("stub executor")
        }
    }

    fn dummy_tool(name: &str, parameters: serde_json::Value) -> AgentTool {
        AgentTool {
            name: name.into(),
            label: name.to_string(),
            description: String::new(),
            parameters,
            executor: Arc::new(DummyExecutor),
            prompt_snippet: None,
            prompt_guidelines: Vec::new(),
            prepare_arguments: None,
            execution_mode: ToolExecutionMode::default(),
            render_call: None,
            render_result: None,
        }
    }

    #[test]
    fn empty_messages() {
        let est = estimate_context_usage(&[], &[]);
        assert_eq!(est.estimated_tokens, 0);
        assert_eq!(est.message_count, 0);
    }

    #[test]
    fn single_text_message() {
        let msg = AgentMessage::User(UserMessage::new("hello"));
        let est = estimate_context_usage(&[msg], &[]);
        assert_eq!(est.estimated_tokens, 1);
        assert_eq!(est.message_count, 1);
    }

    #[test]
    fn exact_multiple_of_chars_per_token() {
        let msg = AgentMessage::User(UserMessage::new("1234"));
        let est = estimate_context_usage(&[msg], &[]);
        assert_eq!(est.estimated_tokens, 1);
    }

    #[test]
    fn rounding_down() {
        let msg = AgentMessage::User(UserMessage::new("12345"));
        let est = estimate_context_usage(&[msg], &[]);
        // 5 / 4 = 1 (integer division)
        assert_eq!(est.estimated_tokens, 1);
    }

    #[test]
    fn tool_parameters_counted() {
        let tool = dummy_tool(
            "read_file",
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        );
        let est = estimate_context_usage(&[], &[tool]);
        let schema_len = serde_json::to_string(&serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}}
        }))
        .unwrap()
        .len();
        assert_eq!(est.estimated_tokens, (schema_len / CHARS_PER_TOKEN) as u64);
    }

    #[test]
    fn combined_messages_and_tools() {
        let msg = AgentMessage::User(UserMessage::new("12345678"));
        let tool = dummy_tool("edit", serde_json::json!({"type": "object"}));
        let est = estimate_context_usage(&[msg], &[tool]);
        let tool_chars = serde_json::to_string(&serde_json::json!({"type": "object"}))
            .unwrap()
            .len();
        assert_eq!(
            est.estimated_tokens,
            ((8 + tool_chars) / CHARS_PER_TOKEN) as u64
        );
    }

    #[test]
    fn assistant_tool_call_arguments_counted() {
        let mut a = AssistantMessage::default();
        a.content.push(AssistantContent::ToolCall(ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: serde_json::Map::from_iter([(
                "path".into(),
                serde_json::Value::String("/tmp".into()),
            )]),
            thought_signature: None,
            r#type: tau_types::ContentBlockType::ToolCall,
        }));
        let msg = AgentMessage::Assistant(a);
        let est = estimate_context_usage(&[msg], &[]);
        // "read_file" = 9, serialized args ~= 14, text = 0 → ~23 chars / 4 = 5
        assert!(est.estimated_tokens >= 5);
    }

    #[test]
    fn bash_execution_command_counted() {
        let msg = AgentMessage::BashExecution(tau_types::BashExecutionMessage {
            role: MessageRole::BashExecution,
            command: "ls -la".into(),
            output: "file1\nfile2".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 0,
            exclude_from_context: false,
        });
        let est = estimate_context_usage(&[msg], &[]);
        // text() returns output "file1\nfile2" = 11, command "ls -la" = 6 → 17 / 4 = 4
        assert_eq!(est.estimated_tokens, 4);
    }

    #[test]
    fn needs_compaction_below_threshold() {
        let est = ContextUsageEstimate {
            estimated_tokens: 100,
            message_count: 1,
        };
        assert!(!needs_compaction(&est, 200_000, DEFAULT_RESERVE));
    }

    #[test]
    fn needs_compaction_at_boundary() {
        let est = ContextUsageEstimate {
            estimated_tokens: 200_000 - DEFAULT_RESERVE,
            message_count: 1,
        };
        assert!(needs_compaction(&est, 200_000, DEFAULT_RESERVE));
    }

    #[test]
    fn needs_compaction_above_threshold() {
        let est = ContextUsageEstimate {
            estimated_tokens: 200_000,
            message_count: 1,
        };
        assert!(needs_compaction(&est, 200_000, DEFAULT_RESERVE));
    }

    #[test]
    fn saturating_sub_prevents_underflow() {
        let est = ContextUsageEstimate {
            estimated_tokens: 100,
            message_count: 1,
        };
        // context_window < reserve → saturating_sub gives 0, anything >= 0 triggers
        assert!(needs_compaction(&est, 50, 100));
    }
}
