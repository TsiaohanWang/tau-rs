use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};
use tau_types::AgentToolResult;

pub struct WriteExecutor {
    cwd: PathBuf,
}

impl WriteExecutor {
    pub fn new(cwd: &Path) -> Self {
        WriteExecutor {
            cwd: cwd.to_path_buf(),
        }
    }
}

#[async_trait]
impl ToolExecutor for WriteExecutor {
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

        let content = arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Missing required argument: content"))?;

        let file_path = if let Some(stripped) = path.strip_prefix('~') {
            let home = dirs::home_dir()
                .ok_or_else(|| ToolError::new("Cannot determine home directory"))?;
            home.join(stripped)
        } else if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };

        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::new(format!("Failed to create directories: {}", e)))?;
        }

        // Atomic write: write to a tempfile in the same directory, then rename.
        // rename(2) is atomic on POSIX when source and target are on the same
        // filesystem, preventing corruption on crash/mid-write.
        let dir = file_path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .map_err(|e| ToolError::new(format!("Failed to create tempfile: {e}")))?;
        std::io::Write::write_all(&mut tmp, content.as_bytes())
            .map_err(|e| ToolError::new(format!("Failed to write tempfile: {e}")))?;
        tmp.persist(&file_path)
            .map_err(|e| ToolError::new(format!("Failed to persist file '{}': {e}", path)))?;

        Ok(AgentToolResult::from_text(format!(
            "Successfully wrote {} bytes to '{}'",
            content.len(),
            path
        )))
    }
}

pub fn create_tool(cwd: &Path) -> AgentTool {
    AgentTool {
        name: "write".into(),
        label: "Write File".to_string(),
        description: "Create or overwrite a file with the given content.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "File path relative to the working directory"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        }),
        executor: Arc::new(WriteExecutor::new(cwd)),
        prompt_snippet: Some(
            "Use this tool to create or overwrite files. Creates parent directories automatically."
                .to_string(),
        ),
        prompt_guidelines: vec![],
        prepare_arguments: None,
        execution_mode: ToolExecutionMode::default(),
        render_call: Some(Arc::new(|args: &Map<String, Value>| {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() {
                None
            } else {
                let lines = content.split('\n').count();
                Some(format!("write {path} ({lines} lines)"))
            }
        })),
        render_result: Some(Arc::new(|result: &AgentToolResult, _expanded: bool| {
            let text = result.text();
            if text.is_empty() { None } else { Some(text) }
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_file() {
        let temp_dir = TempDir::new().unwrap();
        let executor = WriteExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("content".to_string(), json!("hello world"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let text = result.text();

        assert!(text.contains("Successfully wrote"));

        let content = tokio::fs::read_to_string(temp_dir.path().join("test.txt"))
            .await
            .unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn test_write_file_creates_directories() {
        let temp_dir = TempDir::new().unwrap();
        let executor = WriteExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("sub/dir/test.txt"));
        args.insert("content".to_string(), json!("nested file"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        assert!(result.text().contains("Successfully wrote"));

        let content = tokio::fs::read_to_string(temp_dir.path().join("sub/dir/test.txt"))
            .await
            .unwrap();
        assert_eq!(content, "nested file");
    }

    #[tokio::test]
    async fn test_write_file_overwrite() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "original").await.unwrap();

        let executor = WriteExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("content".to_string(), json!("overwritten"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        assert!(result.text().contains("Successfully wrote"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "overwritten");
    }

    #[test]
    fn test_create_tool() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_tool(temp_dir.path());

        assert_eq!(tool.name(), "write");
        assert_eq!(tool.label, "Write File");
    }
}
