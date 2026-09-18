//! Unit tests for [`crate::tau_agent::tools`].
//!
//! Test-only: compiled with `#[cfg(test)]` via `tau_agent::tests`.

use super::support::block_on;
use crate::tau_agent::messages::{ImageContent, TextContent, ToolResultContent};
use crate::tau_agent::tools::*;
use crate::tau_agent::types::JsonObject;
use serde_json::json;
use std::sync::{Arc, Mutex};

fn result_with(text: &str) -> AgentToolResult {
    AgentToolResult {
        content: vec![ToolResultContent::Text(TextContent::new(text))],
        details: None,
        added_tool_names: None,
        terminate: None,
    }
}

/// Expected strings below are produced by `tau_agent.tools` in Python.
#[test]
fn agent_tool_result_matches_python_wire_shape() {
    let full = AgentToolResult {
        content: vec![
            ToolResultContent::Text(TextContent::new("x")),
            ToolResultContent::Image(ImageContent::new("d", "image/png")),
        ],
        details: Some(json!({"a": 1})),
        added_tool_names: Some(vec!["read".to_owned()]),
        terminate: Some(true),
    };
    assert_eq!(
        serde_json::to_string(&full).unwrap(),
        r#"{"content":[{"type":"text","text":"x","textSignature":null},{"type":"image","data":"d","mimeType":"image/png"}],"details":{"a":1},"addedToolNames":["read"],"terminate":true}"#
    );

    let empty = AgentToolResult {
        content: Vec::new(),
        details: None,
        added_tool_names: None,
        terminate: None,
    };
    assert_eq!(
        serde_json::to_string(&empty).unwrap(),
        r#"{"content":[],"details":null,"addedToolNames":null,"terminate":null}"#
    );
}

#[test]
fn agent_tool_result_accepts_snake_case_and_rejects_bad_input() {
    // Python's WireModel sets validate_by_name=True.
    let result: AgentToolResult =
        serde_json::from_str(r#"{"added_tool_names":["x"],"details":7}"#).unwrap();
    assert_eq!(result.added_tool_names, Some(vec!["x".to_owned()]));
    assert_eq!(result.details, Some(json!(7)));
    assert!(result.content.is_empty());

    // Unknown fields are rejected like extra="forbid".
    assert!(serde_json::from_str::<AgentToolResult>(r#"{"nope":1}"#).is_err());
    // Bare string content is a Python before-validator convenience (ADR-006).
    assert!(serde_json::from_str::<AgentToolResult>(r#"{"content":"hi"}"#).is_err());
}

#[test]
fn agent_tool_result_text_skips_images() {
    let result = AgentToolResult {
        content: vec![
            ToolResultContent::Text(TextContent::new("a")),
            ToolResultContent::Image(ImageContent::new("d", "image/png")),
            ToolResultContent::Text(TextContent::new("b")),
        ],
        details: None,
        added_tool_names: None,
        terminate: None,
    };
    assert_eq!(result.text(), "ab");
}

#[test]
fn agent_tool_executes_through_the_boxed_executor() {
    let updates: Arc<Mutex<Vec<AgentToolResult>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&updates);

    let tool = AgentTool::new(
        "read",
        "Read",
        "Read a file",
        JsonObject::from_iter([("type".to_owned(), json!("object"))]),
        Box::new(move |tool_call_id, arguments, _signal, on_update| {
            assert_eq!(tool_call_id, "c1");
            assert_eq!(arguments["path"], json!("x"));
            if let Some(mut on_update) = on_update {
                on_update(result_with("p"));
            }
            let seen = Arc::clone(&seen);
            Box::pin(async move {
                seen.lock().unwrap().push(result_with("seen"));
                result_with("done")
            })
        }),
    );

    assert_eq!(tool.execution_mode, ToolExecutionMode::Parallel);
    assert_eq!(tool.input_schema()["type"], json!("object"));

    let result = block_on(tool.execute(
        "c1".to_owned(),
        JsonObject::from_iter([("path".to_owned(), json!("x"))]),
        None,
        Some(Box::new({
            let updates = Arc::clone(&updates);
            move |partial| updates.lock().unwrap().push(partial)
        })),
    ));

    assert_eq!(result.text(), "done");
    let updates = updates.lock().unwrap();
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].text(), "p");
    assert_eq!(updates[1].text(), "seen");
}

#[test]
fn agent_tool_carries_python_defaults() {
    let tool = AgentTool::new(
        "t",
        "T",
        "d",
        JsonObject::new(),
        Box::new(|_, _, _, _| Box::pin(async { result_with("ok") })),
    );

    assert_eq!(tool.prompt_snippet, None);
    assert!(tool.prompt_guidelines.is_empty());
    assert!(tool.prepare_arguments.is_none());
    assert_eq!(tool.execution_mode, ToolExecutionMode::Parallel);
    assert!(tool.render_call.is_none());
    assert!(tool.render_result.is_none());
}
