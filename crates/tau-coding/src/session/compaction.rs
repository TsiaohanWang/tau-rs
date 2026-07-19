use tau_types::{AgentMessage, AssistantContent, CompactionEntry, new_entry_id};

use crate::compaction_prompts::{
    COMPACTION_SUMMARY_PREFIX, SUMMARIZATION_PROMPT, SUMMARIZATION_SYSTEM_PROMPT,
    SUMMARY_MESSAGE_CHAR_LIMIT, UPDATE_SUMMARIZATION_PROMPT,
};

use super::context_window::estimate_context_usage;

/// Plan for what to compact: which message IDs get summarized.
#[derive(Debug, Clone)]
pub struct CompactionPlan {
    /// Message entry IDs that will be replaced by the summary.
    pub entry_ids: Vec<String>,
    /// The summary text (filled after LLM call).
    pub summary: Option<String>,
}

/// Estimate how many tokens a single message consumes.
fn estimate_message_tokens(msg: &AgentMessage) -> u64 {
    // Reuse the context_window estimator for a single message.
    estimate_context_usage(std::slice::from_ref(msg), &[]).estimated_tokens
}

/// Build a compaction plan: select the oldest N messages that, when removed,
/// free up at least `target_tokens` worth of context.
///
/// `messages` and `entry_ids` must be parallel — `messages[i]` corresponds to
/// `entry_ids[i]`.  Walks from oldest (index 0) to newest, accumulating
/// estimated tokens freed.  Returns `None` if there is nothing to compact
/// (empty input or total context too small).
pub fn plan_compaction(
    messages: &[AgentMessage],
    entry_ids: &[String],
    target_tokens: u64,
) -> Option<CompactionPlan> {
    if messages.is_empty() || messages.len() != entry_ids.len() {
        return None;
    }

    let mut accumulated: u64 = 0;
    let mut compacted_ids: Vec<String> = Vec::new();

    for (msg, id) in messages.iter().zip(entry_ids.iter()) {
        accumulated += estimate_message_tokens(msg);
        compacted_ids.push(id.clone());
        if accumulated >= target_tokens {
            return Some(CompactionPlan {
                entry_ids: compacted_ids,
                summary: None,
            });
        }
    }

    // Only return a plan when we actually freed enough tokens.
    if accumulated >= target_tokens {
        Some(CompactionPlan {
            entry_ids: compacted_ids,
            summary: None,
        })
    } else {
        None
    }
}

/// Create a `CompactionEntry` from a plan (to be appended to session storage).
///
/// The plan's `summary` must already be filled in by the caller after the LLM
/// call.  Panics if `summary` is `None`.
pub fn create_compaction_entry(plan: &CompactionPlan) -> CompactionEntry {
    let summary = plan
        .summary
        .clone()
        .expect("plan summary must be set before creating a CompactionEntry");
    CompactionEntry {
        id: new_entry_id(),
        parent_id: None,
        timestamp: tau_types::current_timestamp_secs(),
        r#type: tau_types::EntryType::Compaction,
        summary,
        replaces_entry_ids: plan.entry_ids.clone(),
    }
}

// ---------------------------------------------------------------------------
// Compaction summary prompt construction
// ---------------------------------------------------------------------------

/// Serialize a single message for inclusion in the compaction prompt.
///
/// Mirrors Python's `serialize_messages_for_compaction`. Each message is
/// wrapped in `<message index=N role=...>` tags, with tool calls and text
/// content placed inside. Output is truncated to
/// [`SUMMARY_MESSAGE_CHAR_LIMIT`] characters per message.
fn serialize_message_for_compaction(index: usize, msg: &AgentMessage) -> String {
    let role = match msg {
        AgentMessage::User(_) => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "tool",
        AgentMessage::BashExecution(_) => "bash",
        _ => "unknown",
    };

    let mut attrs = format!("index={index} role={role}");

    if let AgentMessage::ToolResult(t) = msg {
        attrs += &format!(" name={} error={}", t.tool_name, t.is_error);
    }

    let mut inner = String::new();

    let text = msg.text();
    if !text.is_empty() {
        inner.push_str(&truncate_for_summary(&text));
        inner.push('\n');
    }

    // Append tool calls for assistant messages.
    if let AgentMessage::Assistant(a) = msg {
        let tool_calls: Vec<String> = a
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantContent::ToolCall(tc) => Some(format!(
                    "- {}: {}",
                    tc.name,
                    serde_json::Value::Object(tc.arguments.clone())
                )),
                _ => None,
            })
            .collect();
        if !tool_calls.is_empty() {
            inner.push_str("<tool-calls>\n");
            for tc in &tool_calls {
                inner.push_str(tc);
                inner.push('\n');
            }
            inner.push_str("</tool-calls>\n");
        }
    }

    format!("<message {attrs}>\n{inner}</message>")
}

/// Build the full user-prompt text sent to the summarization model.
///
/// Mirrors Python's `build_compaction_summary_prompt`. Detects whether the
/// first message is an existing compaction summary and uses the appropriate
/// base prompt (`SUMMARIZATION_PROMPT` vs `UPDATE_SUMMARIZATION_PROMPT`).
pub fn build_compaction_summary_prompt(messages: &[AgentMessage]) -> String {
    let (previous_summary, new_messages) = split_previous_compaction_summary(messages);

    let conversation = serialize_messages_for_compaction(new_messages);
    let mut prompt = format!("<conversation>\n{conversation}\n</conversation>\n\n");

    let base_prompt = if previous_summary.is_some() {
        UPDATE_SUMMARIZATION_PROMPT
    } else {
        SUMMARIZATION_PROMPT
    };

    if let Some(ref summary) = previous_summary {
        prompt += &format!("<previous-summary>\n{summary}\n</previous-summary>\n\n");
    }

    prompt += base_prompt;
    prompt
}

/// Return the summarization system prompt.
pub fn summarization_system_prompt() -> &'static str {
    SUMMARIZATION_SYSTEM_PROMPT
}

/// Serialize a slice of messages into the compaction-prompt format.
///
/// Each message is wrapped in `<message>` tags. Returns
/// `"(no new messages)"` when the input is empty.
fn serialize_messages_for_compaction(messages: &[AgentMessage]) -> String {
    if messages.is_empty() {
        return "(no new messages)".to_string();
    }

    messages
        .iter()
        .enumerate()
        .map(|(i, msg)| serialize_message_for_compaction(i + 1, msg))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check whether the first message is an existing compaction summary and
/// split it off from the rest of the messages.
///
/// Returns `(previous_summary, remaining_messages)`. If there is no existing
/// summary the first element is `None` and all messages are returned as-is.
fn split_previous_compaction_summary(
    messages: &[AgentMessage],
) -> (Option<String>, &[AgentMessage]) {
    if messages.is_empty() {
        return (None, messages);
    }

    let first = &messages[0];
    if !matches!(first, AgentMessage::User(_)) {
        return (None, messages);
    }

    let text = first.text();
    if !text.starts_with(COMPACTION_SUMMARY_PREFIX) {
        return (None, messages);
    }

    let summary = text[COMPACTION_SUMMARY_PREFIX.len()..].to_string();
    (Some(summary), &messages[1..])
}

/// Truncate text for inclusion in a compaction summary message.
fn truncate_for_summary(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.len() <= SUMMARY_MESSAGE_CHAR_LIMIT {
        return collapsed;
    }
    let truncated: String = collapsed
        .chars()
        .take(SUMMARY_MESSAGE_CHAR_LIMIT - 3)
        .collect();
    // Trim to a valid UTF-8 boundary.
    let truncated = truncated.trim_end().to_string();
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::UserMessage;

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::User(UserMessage::new(text))
    }

    #[test]
    fn plan_compaction_empty_messages() {
        assert!(plan_compaction(&[], &[], 100).is_none());
    }

    #[test]
    fn plan_compaction_mismatched_lengths() {
        let msgs = vec![user_msg("hi")];
        assert!(plan_compaction(&msgs, &[], 100).is_none());
    }

    #[test]
    fn plan_compaction_returns_none_when_total_below_target() {
        // "hi" = 2 chars → 0 tokens (2/4 = 0), so nothing gets freed.
        let msgs = vec![user_msg("hi")];
        let ids = vec!["id1".to_string()];
        assert!(plan_compaction(&msgs, &ids, 1).is_none());
    }

    #[test]
    fn plan_compaction_collects_enough_messages() {
        // "12345678" = 8 chars → 2 tokens each.  Target 5 tokens needs 3 msgs.
        let msgs = vec![
            user_msg("12345678"),
            user_msg("12345678"),
            user_msg("12345678"),
        ];
        let ids: Vec<String> = (0..3).map(|i| format!("id{i}")).collect();
        let plan = plan_compaction(&msgs, &ids, 5).expect("should return plan");
        assert_eq!(plan.entry_ids.len(), 3);
        assert_eq!(plan.entry_ids[0], "id0");
        assert_eq!(plan.entry_ids[2], "id2");
        assert!(plan.summary.is_none());
    }

    #[test]
    fn plan_compaction_stops_early_when_target_reached() {
        // Two 8-char messages = 2 tokens each.  Target 3 tokens → need 2 msgs.
        let msgs = vec![user_msg("12345678"), user_msg("12345678")];
        let ids: Vec<String> = (0..2).map(|i| format!("id{i}")).collect();
        let plan = plan_compaction(&msgs, &ids, 3).expect("should return plan");
        assert_eq!(plan.entry_ids.len(), 2);
    }

    #[test]
    fn plan_compaction_all_messages_when_insufficient() {
        // Total context is 4 tokens but target is 100 → returns None.
        let msgs = vec![user_msg("12345678"), user_msg("12345678")];
        let ids: Vec<String> = (0..2).map(|i| format!("id{i}")).collect();
        assert!(plan_compaction(&msgs, &ids, 100).is_none());
    }

    #[test]
    fn create_compaction_entry_uses_plan_ids() {
        let plan = CompactionPlan {
            entry_ids: vec!["a".into(), "b".into()],
            summary: Some("conversation about Rust generics".into()),
        };
        let entry = create_compaction_entry(&plan);
        assert_eq!(entry.replaces_entry_ids, vec!["a", "b"]);
        assert_eq!(entry.summary, "conversation about Rust generics");
        assert_eq!(entry.r#type, tau_types::EntryType::Compaction);
        // entry.id should be a fresh UUID (non-empty hex).
        assert!(!entry.id.is_empty());
    }

    #[test]
    #[should_panic(expected = "plan summary must be set")]
    fn create_compaction_entry_panics_without_summary() {
        let plan = CompactionPlan {
            entry_ids: vec!["a".into()],
            summary: None,
        };
        let _ = create_compaction_entry(&plan);
    }
}
