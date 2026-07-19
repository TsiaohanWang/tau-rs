use tau_agent::tool::AgentTool;

const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful coding assistant.";

pub fn build_system_prompt(tools: &[AgentTool], user_system: &str) -> String {
    let mut prompt = if user_system.trim().is_empty() {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        user_system.to_string()
    };

    prompt.push_str("\n\n## Available Tools\n");

    for tool in tools {
        prompt.push_str(&format!("\n### {} ({})\n", tool.label, tool.name()));

        prompt.push_str(&format!("\n{}\n", tool.description));

        if let Some(ref snippet) = tool.prompt_snippet {
            prompt.push_str(&format!("\n{}\n", snippet));
        }

        if !tool.prompt_guidelines.is_empty() {
            prompt.push_str("\n**Guidelines:**\n");
            for guideline in &tool.prompt_guidelines {
                prompt.push_str(&format!("- {}\n", guideline));
            }
        }
    }

    prompt
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn fake_tool(
        name: &str,
        label: &str,
        snippet: Option<String>,
        guidelines: Vec<String>,
    ) -> AgentTool {
        AgentTool {
            name: name.into(),
            label: label.to_string(),
            description: format!("Description of {}.", name),
            parameters: serde_json::json!({}),
            executor: Arc::new(DummyExecutor),
            prompt_snippet: snippet,
            prompt_guidelines: guidelines,
            prepare_arguments: None,
            execution_mode: tau_agent::tool::ToolExecutionMode::default(),
            render_call: None,
            render_result: None,
        }
    }

    struct DummyExecutor;

    #[async_trait::async_trait]
    impl tau_agent::tool::ToolExecutor for DummyExecutor {
        async fn execute(
            &self,
            _tool_call_id: &str,
            _arguments: &serde_json::Map<String, serde_json::Value>,
            _signal: Option<tokio_util::sync::CancellationToken>,
            _on_update: Option<&(dyn Fn(tau_types::AgentToolResult) + Send + Sync)>,
        ) -> Result<tau_types::AgentToolResult, tau_agent::tool::ToolError> {
            unreachable!("dummy executor")
        }
    }

    #[test]
    fn test_empty_user_prompt_uses_default() {
        let tools = vec![];
        let result = build_system_prompt(&tools, "");
        assert!(result.starts_with(DEFAULT_SYSTEM_PROMPT));
    }

    #[test]
    fn test_user_prompt_is_preserved() {
        let tools = vec![];
        let result = build_system_prompt(&tools, "Be concise.");
        assert!(result.starts_with("Be concise."));
    }

    #[test]
    fn test_single_tool_with_snippet_and_guidelines() {
        let tools = vec![fake_tool(
            "read",
            "Read File",
            Some("Reads files.".to_string()),
            vec!["Use paths relative to cwd.".to_string()],
        )];

        let result = build_system_prompt(&tools, "You are helpful.");

        assert!(result.contains("## Available Tools"));
        assert!(result.contains("### Read File (read)"));
        assert!(result.contains("Description of read."));
        assert!(result.contains("Reads files."));
        assert!(result.contains("**Guidelines:**"));
        assert!(result.contains("- Use paths relative to cwd."));
    }

    #[test]
    fn test_tool_without_snippet_omits_snippet_line() {
        let tools = vec![fake_tool("write", "Write File", None, vec![])];

        let result = build_system_prompt(&tools, "You are helpful.");

        assert!(result.contains("### Write File (write)"));
        assert!(result.contains("Description of write."));
        // No snippet or guidelines section
        assert!(!result.contains("**Guidelines:**"));
    }

    #[test]
    fn test_multiple_tools() {
        let tools = vec![
            fake_tool(
                "read",
                "Read File",
                Some("Reads files.".to_string()),
                vec!["Read carefully.".to_string()],
            ),
            fake_tool(
                "bash",
                "Bash Command",
                Some("Runs commands.".to_string()),
                vec!["Be careful.".to_string(), "Use timeouts.".to_string()],
            ),
        ];

        let result = build_system_prompt(&tools, "");

        assert!(result.contains("### Read File (read)"));
        assert!(result.contains("### Bash Command (bash)"));
        assert!(result.contains("- Read carefully."));
        assert!(result.contains("- Be careful."));
        assert!(result.contains("- Use timeouts."));
    }
}
