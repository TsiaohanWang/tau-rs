use tau_types::{AgentMessage, CompactionEntry, new_entry_id};

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
