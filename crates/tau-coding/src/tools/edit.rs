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

/// Strip BOM from the front of a string, returning `(had_bom, content_without_bom)`.
fn strip_bom(s: &str) -> (bool, &str) {
    if let Some(stripped) = s.strip_prefix('\u{FEFF}') {
        (true, stripped)
    } else {
        (false, s)
    }
}

/// Normalize line endings: convert `\r\n` → `\n` for matching purposes.
fn normalize_lf(s: &str) -> String {
    s.replace("\r\n", "\n")
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

        let file_path = if let Some(stripped) = path.strip_prefix('~') {
            let home = dirs::home_dir()
                .ok_or_else(|| ToolError::new("Cannot determine home directory"))?;
            home.join(stripped)
        } else if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };

        let raw_content = tokio::fs::read_to_string(&file_path)
            .await
            .map_err(|e| ToolError::new(format!("Failed to read file '{}': {}", path, e)))?;

        let (had_bom, content_no_bom) = strip_bom(&raw_content);
        let normalized_content = normalize_lf(content_no_bom);
        let normalized_old = normalize_lf(old_text);
        let normalized_new = normalize_lf(new_text);

        let occurrences = normalized_content.matches(&*normalized_old).count();

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

        let new_content_owned = normalized_content.replace(&*normalized_old, &normalized_new);

        // Re-add BOM if the original had one
        let final_content = if had_bom {
            format!("\u{FEFF}{new_content_owned}")
        } else {
            new_content_owned.clone()
        };

        // Atomic write
        let dir = file_path.parent().unwrap_or(Path::new("."));
        let mut tmp = tempfile::NamedTempFile::new_in(dir)
            .map_err(|e| ToolError::new(format!("Failed to create tempfile: {e}")))?;
        std::io::Write::write_all(&mut tmp, final_content.as_bytes())
            .map_err(|e| ToolError::new(format!("Failed to write tempfile: {e}")))?;
        tmp.persist(&file_path)
            .map_err(|e| ToolError::new(format!("Failed to persist file '{}': {e}", path)))?;

        // Generate diff for the result
        let diff = similar::TextDiff::from_lines(&normalized_content, &new_content_owned);
        let mut diff_text = String::new();
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                similar::ChangeTag::Delete => "-",
                similar::ChangeTag::Insert => "+",
                similar::ChangeTag::Equal => continue,
            };
            diff_text.push_str(sign);
            diff_text.push_str(change.as_str().unwrap_or(""));
        }

        let result_msg = if diff_text.is_empty() {
            format!("Successfully edited '{}' (no net change)", path)
        } else {
            format!("Successfully edited '{}'\n\n{}", path, diff_text)
        };

        Ok(AgentToolResult::from_text(result_msg))
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

    #[tokio::test]
    async fn test_edit_normalizes_crlf() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "hello\r\nworld\r\n")
            .await
            .unwrap();

        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("old_text".to_string(), json!("hello\nworld"));
        args.insert("new_text".to_string(), json!("goodbye\nworld"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        assert!(result.text().contains("Successfully edited"));

        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        // After normalize+replace, the file uses \n line endings
        assert_eq!(content, "goodbye\nworld\n");
    }

    #[tokio::test]
    async fn test_edit_preserves_bom() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "\u{FEFF}hello world")
            .await
            .unwrap();

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
        assert!(content.starts_with('\u{FEFF}'));
        assert_eq!(content, "\u{FEFF}goodbye world");
    }

    #[tokio::test]
    async fn test_edit_returns_diff() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        tokio::fs::write(&file_path, "line1\nline2\nline3")
            .await
            .unwrap();

        let executor = EditExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("path".to_string(), json!("test.txt"));
        args.insert("old_text".to_string(), json!("line2"));
        args.insert("new_text".to_string(), json!("LINE2"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let text = result.text();
        assert!(text.contains("-line2"));
        assert!(text.contains("+LINE2"));
    }

    #[test]
    fn test_create_tool() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_tool(temp_dir.path());

        assert_eq!(tool.name(), "edit");
        assert_eq!(tool.label, "Edit File");
    }
}
