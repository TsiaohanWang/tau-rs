use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};
use tau_types::AgentToolResult;

pub struct ReadExecutor {
    cwd: PathBuf,
}

impl ReadExecutor {
    pub fn new(cwd: &Path) -> Self {
        ReadExecutor {
            cwd: cwd.to_path_buf(),
        }
    }
}

#[async_trait]
impl ToolExecutor for ReadExecutor {
    async fn execute(
        &self,
        _tool_call_id: &str,
        arguments: &Map<String, Value>,
        _signal: Option<CancellationToken>,
        _on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> Result<AgentToolResult, ToolError> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Missing required argument: path"))?;

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(2000) as usize;

        // Resolve path relative to cwd
        let file_path = if let Some(stripped) = path.strip_prefix('~') {
            let home = dirs::home_dir()
                .ok_or_else(|| ToolError::new("Cannot determine home directory"))?;
            home.join(stripped)
        } else if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };

        // Read file content
        let content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| ToolError::new(format!("Failed to read file '{}': {}", path, e)))?;

        // Apply offset and limit
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        if offset >= total_lines {
            return Ok(AgentToolResult::from_text(format!(
                "File '{}' is empty or offset {} is beyond end of file ({} lines)",
                path, offset, total_lines
            )));
        }

        let end = (offset + limit).min(total_lines);
        let selected_lines = &lines[offset..end];

        // Format with line numbers
        let mut output = String::new();
        for (i, line) in selected_lines.iter().enumerate() {
            let line_num = offset + i + 1;
            output.push_str(&format!("{}: {}\n", line_num, line));
        }

        if end < total_lines {
            output.push_str(&format!(
                "\n... ({} more lines, showing {} of {})",
                total_lines - end,
                end - offset,
                total_lines
            ));
        }

        Ok(AgentToolResult::from_text(output))
    }
}

pub fn create_tool(cwd: &Path) -> AgentTool {
    AgentTool {
        name: "read".into(),
        label: "Read File".to_string(),
        description: "Read the contents of a file with line numbers.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the working directory"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (0-indexed)",
                    "default": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of lines to read",
                    "default": 2000
                }
            },
            "required": ["path"]
        }),
        executor: Arc::new(ReadExecutor::new(cwd)),
        prompt_snippet: Some(
            "Use this tool to read files. Returns file contents with line numbers.".to_string(),
        ),
        prompt_guidelines: vec![
            "Always specify path relative to the working directory".to_string(),
            "Use offset/limit for large files to avoid reading everything".to_string(),
            "Check the first few lines before reading the entire file".to_string(),
        ],
        prepare_arguments: None,
        execution_mode: ToolExecutionMode::default(),
        render_call: None,
        render_result: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line1\nline2\nline3\n")
            .await
            .unwrap();

        let executor = ReadExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let text = result.text();

        assert!(text.contains("1: line1"));
        assert!(text.contains("2: line2"));
        assert!(text.contains("3: line3"));
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let executor = ReadExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("nonexistent.txt"));

        let result = executor.execute("test-id", &args, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_file_with_offset() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line1\nline2\nline3\nline4\nline5\n")
            .await
            .unwrap();

        let executor = ReadExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("offset".to_string(), json!(2));
        args.insert("limit".to_string(), json!(2));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let text = result.text();

        assert!(text.contains("3: line3"));
        assert!(text.contains("4: line4"));
        assert!(!text.contains("1: line1"));
        assert!(!text.contains("5: line5"));
    }

    #[test]
    fn test_create_tool() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_tool(temp_dir.path());

        assert_eq!(tool.name(), "read");
        assert_eq!(tool.label, "Read File");
        assert!(tool.description.contains("Read the contents"));
    }
}
