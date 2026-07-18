use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};
use tau_types::AgentToolResult;

pub struct EditExecutor {
    cwd: PathBuf,
}

impl EditExecutor {
    pub fn new(cwd: &Path) -> Self {
        EditExecutor {
            cwd: cwd.to_path_buf(),
        }
    }
}

#[async_trait]
impl ToolExecutor for EditExecutor {
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

        let old_text = arguments
            .get("old_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Missing required argument: old_text"))?;

        let new_text = arguments
            .get("new_text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Missing required argument: new_text"))?;

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

        // Check if old_text exists in the file
        let occurrences = content.matches(old_text).count();

        if occurrences == 0 {
            return Err(ToolError::new(format!(
                "old_text not found in file '{}'",
                path
            )));
        }

        if occurrences > 1 {
            return Err(ToolError::new(format!(
                "old_text found {} times in file '{}'. Please provide more context to make it unique.",
                occurrences, path
            )));
        }

        // Replace old_text with new_text
        let new_content = content.replace(old_text, new_text);

        // Write back to file
        tokio::fs::write(&file_path, &new_content)
            .await
            .map_err(|e| ToolError::new(format!("Failed to write file '{}': {}", path, e)))?;

        Ok(AgentToolResult::from_text(format!(
            "Successfully edited '{}'",
            path
        )))
    }
}

pub fn create_tool(cwd: &Path) -> AgentTool {
    AgentTool {
        name: "edit".into(),
        label: "Edit File".to_string(),
        description: "Edit a file by replacing specific text.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the working directory"
                },
                "old_text": {
                    "type": "string",
                    "description": "Text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "Text to replace with"
                }
            },
            "required": ["path", "old_text", "new_text"]
        }),
        executor: Arc::new(EditExecutor::new(cwd)),
        prompt_snippet: Some("Use this tool to edit files by replacing specific text.".to_string()),
        prompt_guidelines: vec![
            "old_text must be unique in the file".to_string(),
            "old_text must match exactly (including whitespace and indentation)".to_string(),
            "If the edit fails, the file is NOT modified".to_string(),
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
    async fn test_edit_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world").await.unwrap();

        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("old_text".to_string(), json!("hello"));
        args.insert("new_text".to_string(), json!("goodbye"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        assert!(result.text().contains("Successfully edited"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "goodbye world");
    }

    #[tokio::test]
    async fn test_edit_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("nonexistent.txt"));
        args.insert("old_text".to_string(), json!("hello"));
        args.insert("new_text".to_string(), json!("goodbye"));

        let result = executor.execute("test-id", &args, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_edit_file_old_text_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello world").await.unwrap();

        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("old_text".to_string(), json!("goodbye"));
        args.insert("new_text".to_string(), json!("hello"));

        let result = executor.execute("test-id", &args, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("not found"));
    }

    #[tokio::test]
    async fn test_edit_file_multiple_occurrences() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello hello hello")
            .await
            .unwrap();

        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("old_text".to_string(), json!("hello"));
        args.insert("new_text".to_string(), json!("goodbye"));

        let result = executor.execute("test-id", &args, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("found 3 times"));
    }

    #[test]
    fn test_create_tool() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_tool(temp_dir.path());

        assert_eq!(tool.name(), "edit");
        assert_eq!(tool.label, "Edit File");
    }
}
