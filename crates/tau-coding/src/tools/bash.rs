use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};
use tau_types::AgentToolResult;

pub struct BashExecutor {
    cwd: PathBuf,
}

impl BashExecutor {
    pub fn new(cwd: &Path) -> Self {
        BashExecutor {
            cwd: cwd.to_path_buf(),
        }
    }
}

#[async_trait]
impl ToolExecutor for BashExecutor {
    async fn execute(
        &self,
        _tool_call_id: &str,
        arguments: &Map<String, Value>,
        signal: Option<CancellationToken>,
        _on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
    ) -> Result<AgentToolResult, ToolError> {
        let command = arguments
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::new("Missing required argument: command"))?;

        let timeout = arguments
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120);

        let max_output_bytes = 100 * 1024; // 100KB

        // Create process with process group for proper cleanup
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .process_group(0)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // Set up cancellation
        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::new(format!("Failed to execute command: {}", e)))?;

        // Take stdout and stderr before moving child
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();

        // Spawn a task to handle cancellation
        let cancel_handle = if let Some(signal) = signal {
            let child_id = child.id();
            Some(tokio::spawn(async move {
                signal.cancelled().await;
                if let Some(pid) = child_id {
                    unsafe {
                        libc::killpg(pid as i32, libc::SIGTERM);
                    }
                }
            }))
        } else {
            None
        };

        // Wait for completion with timeout
        let output = tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout)) => {
                let _ = child.kill().await;
                return Err(ToolError::new(format!("Command timed out after {} seconds", timeout)));
            }
            result = child.wait() => {
                let status = result.map_err(|e| ToolError::new(format!("Failed to wait for command: {}", e)))?;

                // Read remaining output after process exits
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();

                use tokio::io::AsyncReadExt;
                let _ = stdout.read_to_end(&mut stdout_buf).await;
                let _ = stderr.read_to_end(&mut stderr_buf).await;

                std::process::Output {
                    status,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                }
            }
        };

        // Clean up cancel handle
        if let Some(handle) = cancel_handle {
            handle.abort();
        }

        // Process output
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let mut output_text = String::new();
        let mut truncated = false;

        if !stdout.is_empty() {
            if stdout.len() > max_output_bytes {
                output_text.push_str(&stdout[..max_output_bytes]);
                output_text.push_str("\n... (output truncated)");
                truncated = true;
            } else {
                output_text.push_str(&stdout);
            }
        }

        if !stderr.is_empty() {
            if !output_text.is_empty() {
                output_text.push('\n');
            }
            if stderr.len() > max_output_bytes {
                output_text.push_str(&stderr[..max_output_bytes]);
                output_text.push_str("\n... (stderr truncated)");
                truncated = true;
            } else {
                output_text.push_str(&stderr);
            }
        }

        let exit_code = output.status.code().unwrap_or(-1);

        let mut result = AgentToolResult::from_text(output_text);
        result.details = json!({
            "command": command,
            "exit_code": exit_code,
            "truncated": truncated,
        });

        Ok(result)
    }
}

pub fn create_tool(cwd: &Path) -> AgentTool {
    AgentTool {
        name: "bash".into(),
        label: "Bash Command".to_string(),
        description: "Execute a shell command and return the output.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 120)",
                    "default": 120
                }
            },
            "required": ["command"]
        }),
        executor: Arc::new(BashExecutor::new(cwd)),
        prompt_snippet: Some(
            "Use this tool to execute shell commands. Returns output and exit code.".to_string(),
        ),
        prompt_guidelines: vec![
            "Commands run in the working directory".to_string(),
            "Long-running commands will be killed after timeout".to_string(),
            "Use this for building, testing, running scripts, etc.".to_string(),
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
    async fn test_bash_command() {
        let temp_dir = TempDir::new().unwrap();
        let executor = BashExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("command".to_string(), json!("echo hello"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let text = result.text();

        assert!(text.contains("hello"));
    }

    #[tokio::test]
    async fn test_bash_command_with_exit_code() {
        let temp_dir = TempDir::new().unwrap();
        let executor = BashExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("command".to_string(), json!("exit 1"));

        let result = executor
            .execute("test-id", &args, None, None)
            .await
            .unwrap();
        let details = result.details.clone();

        assert_eq!(details["exit_code"], 1);
    }

    #[tokio::test]
    async fn test_bash_command_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let executor = BashExecutor::new(temp_dir.path());
        let mut args = Map::new();
        args.insert("command".to_string(), json!("sleep 10"));
        args.insert("timeout".to_string(), json!(1));

        let result = executor.execute("test-id", &args, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("timed out"));
    }

    #[test]
    fn test_create_tool() {
        let temp_dir = TempDir::new().unwrap();
        let tool = create_tool(temp_dir.path());

        assert_eq!(tool.name(), "bash");
        assert_eq!(tool.label, "Bash Command");
    }
}
