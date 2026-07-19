//! Interrupted-tool-call repair — mirrors Python `session.py`'s
//! `_repair_interrupted_tool_calls` in-memory step.
//!
//! When a session is loaded — possibly after a crash or a cancellation — any
//! assistant `ToolCall`s whose corresponding `ToolResultMessage` is missing
//! leave the conversation in an inconsistent state the provider would refuse
//! (it expects every `tool_use` to have a matching `tool_result`). The repair
//! inserts synthetic "interrupted" error `ToolResultMessage`s after the
//! assistant message that issued the orphan tool call.
//!
//! The repair is purely in-memory: nothing is written to disk. Python's
//! session loader does the same; persisting synthetic results would corrupt
//! the session tree.

use std::collections::HashSet;

use tau_types::{
    AgentMessage, AssistantContent, TextContent, ToolCall, ToolResultContent, ToolResultMessage,
};

/// Scan `messages` for any `Assistant` message containing a `ToolCall` whose
/// `id` is not matched by a subsequent `ToolResultMessage` (by `tool_call_id`).
/// For each orphan, insert a synthetic error `ToolResultMessage` immediately
/// after the assistant message that issued the call.
///
/// Returns the synthesized `tool_call_id`s (in insertion order) so callers
/// can assert on the repair in tests.
pub fn repair_interrupted_tool_calls(messages: &mut Vec<AgentMessage>) -> Vec<String> {
    // First pass: collect every tool_call_id that already has a ToolResult.
    let mut answered: HashSet<String> = HashSet::new();
    for msg in messages.iter() {
        if let AgentMessage::ToolResult(t) = msg {
            answered.insert(t.tool_call_id.clone());
        }
    }

    // Second pass: walk through and, for any assistant tool_call whose id
    // isn't in `answered`, synthesize an interrupted result. Insertions go
    // right after the assistant message and any other content the assistant
    // streams; a single assistant message is one block in the list.
    let mut repaired: Vec<String> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let orphan_ids: Vec<ToolCall> = match &messages[i] {
            AgentMessage::Assistant(a) => a
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantContent::ToolCall(tc) => Some(tc.clone()),
                    _ => None,
                })
                .filter(|tc| !answered.contains(&tc.id))
                .collect(),
            _ => Vec::new(),
        };

        if orphan_ids.is_empty() {
            i += 1;
            continue;
        }

        // Insert one synthetic ToolResultMessage per orphan tool_call, just
        // after position i. Each insert shifts later indices by +1.
        let mut insert_at = i + 1;
        for tc in orphan_ids {
            let mut result = ToolResultMessage::new(tc.id.clone(), tc.name.clone());
            result.is_error = true;
            result
                .content
                .push(ToolResultContent::Text(TextContent::new(
                    "[Interrupted — tool did not finish before the session was reloaded]",
                )));
            messages.insert(insert_at, AgentMessage::ToolResult(result));
            answered.insert(tc.id.clone());
            repaired.push(tc.id.clone());
            insert_at += 1;
        }

        // Skip the assistant + new synthetic results.
        i = insert_at;
    }

    repaired
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::{AssistantMessage, ContentBlockType};

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(tau_types::UserMessage::new(text))
    }

    fn assistant_with_tool_call(call_id: &str, name: &str) -> AgentMessage {
        let mut a = AssistantMessage::default();
        a.content.push(AssistantContent::Text(TextContent::new(
            "let me check something",
        )));
        a.content.push(AssistantContent::ToolCall(ToolCall {
            id: call_id.to_string(),
            name: name.to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
            r#type: ContentBlockType::ToolCall,
        }));
        AgentMessage::Assistant(a)
    }

    fn tool_result(call_id: &str, name: &str) -> AgentMessage {
        let mut t = ToolResultMessage::new(call_id.to_string(), name.to_string());
        t.content
            .push(ToolResultContent::Text(TextContent::new("ok")));
        AgentMessage::ToolResult(t)
    }

    #[test]
    fn passthrough_when_no_orphans() {
        let msgs = vec![
            user_msg("hi"),
            assistant_with_tool_call("c1", "read"),
            tool_result("c1", "read"),
        ];
        let mut original = msgs.clone();
        let repaired = repair_interrupted_tool_calls(&mut original);
        assert!(repaired.is_empty());
        assert_eq!(original, msgs);
    }

    #[test]
    fn synthesizes_one_result_per_orphan_tool_call() {
        let assistant = assistant_with_tool_call("c1", "read");
        let mut msgs = vec![user_msg("hi"), assistant.clone()];
        let repaired = repair_interrupted_tool_calls(&mut msgs);
        assert_eq!(repaired, vec!["c1".to_string()]);
        // 2 → 3 (assistant + synthetic tool_result)
        assert_eq!(msgs.len(), 3);
        let inserted = match &msgs[2] {
            AgentMessage::ToolResult(t) => t,
            other => panic!("expected ToolResult, got {other:?}"),
        };
        assert_eq!(inserted.tool_call_id, "c1");
        assert_eq!(inserted.tool_name, "read");
        assert!(inserted.is_error);
        assert!(inserted.text().contains("Interrupted"));
    }

    #[test]
    fn multiple_orphans_in_one_assistant() {
        let mut a = AssistantMessage::default();
        a.content
            .push(AssistantContent::Text(TextContent::new("two calls")));
        for id in ["c1", "c2"] {
            a.content.push(AssistantContent::ToolCall(ToolCall {
                id: id.to_string(),
                name: "bash".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
                r#type: ContentBlockType::ToolCall,
            }));
        }
        let mut msgs = vec![user_msg("go"), AgentMessage::Assistant(a)];
        let repaired = repair_interrupted_tool_calls(&mut msgs);
        assert_eq!(repaired, vec!["c1".to_string(), "c2".to_string()]);
        // 2 → 4 (assistant + 2 synthetic tool_results)
        assert_eq!(msgs.len(), 4);
        // Synthetic results appear in tool_call order.
        match (&msgs[2], &msgs[3]) {
            (AgentMessage::ToolResult(t1), AgentMessage::ToolResult(t2)) => {
                assert_eq!(t1.tool_call_id, "c1");
                assert_eq!(t2.tool_call_id, "c2");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn already_answered_calls_are_left_alone() {
        let assistant = assistant_with_tool_call("c1", "read");
        let result = tool_result("c1", "read");
        let mut msgs = vec![user_msg("hi"), assistant, result];
        let repaired = repair_interrupted_tool_calls(&mut msgs);
        assert!(repaired.is_empty());
        assert_eq!(msgs.len(), 3);
    }
}
