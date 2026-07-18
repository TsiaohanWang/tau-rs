//! In-memory session state reconstruction — `tau_agent.session.memory`.
//!
//! Replays append-only `SessionEntry` journal into a live `SessionState`,
//! applying compaction and formatting branch/compaction summary messages.

use tau_types::{
    AgentMessage, BranchSummaryEntry, CompactionEntry, CustomEntry, SessionEntry, SessionInfoEntry,
    UserMessage,
};

use crate::session::tree::path_to_entry;

/// Selects which entries to replay, mirroring Python's `_UNSET_LEAF_ID` sentinel:
///
/// - `Linear`: replay all entries in storage order (picks up the last
///   `LeafEntry`'s pointer as the active leaf).
/// - `At(None)`: replay the empty path before the first root entry.
/// - `At(Some(id))`: replay the root-to-leaf path to `id`.
#[derive(Debug, Clone, Copy)]
pub enum LeafSelector<'a> {
    Linear,
    At(Option<&'a str>),
}

#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub messages: Vec<AgentMessage>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub label: Option<String>,
    pub active_leaf_id: Option<String>,
    pub session_info: Option<SessionInfoEntry>,
    pub custom_entries: Vec<CustomEntry>,
    pub compaction_entries: Vec<CompactionEntry>,
    pub context_entry_ids: Vec<String>,
    pub entries: Vec<SessionEntry>,
}

impl SessionState {
    pub fn from_entries(
        entries: &[SessionEntry],
        leaf: LeafSelector<'_>,
    ) -> Result<SessionState, crate::session::tree::SessionTreeError> {
        let (replay, resolved_leaf): (Vec<&SessionEntry>, Option<Option<String>>) = match leaf {
            LeafSelector::Linear => (entries.iter().collect(), Some(None)),
            LeafSelector::At(None) => (Vec::new(), Some(None)),
            LeafSelector::At(Some(id)) => (path_to_entry(entries, id)?, Some(Some(id.to_string()))),
        };
        let resolved_leaf = resolved_leaf.unwrap_or(None);

        let mut message_rows: Vec<(String, AgentMessage)> = Vec::new();
        let mut model: Option<String> = None;
        let mut thinking_level: Option<String> = None;
        let mut label: Option<String> = None;
        let mut active_leaf_id: Option<String> = resolved_leaf.clone();
        let mut session_info: Option<SessionInfoEntry> = None;
        let mut custom_entries: Vec<CustomEntry> = Vec::new();
        let mut compaction_entries: Vec<CompactionEntry> = Vec::new();

        for entry in &replay {
            match entry {
                SessionEntry::Message(e) => {
                    message_rows.push((e.id.clone(), e.message.clone()));
                }
                SessionEntry::ModelChange(e) => {
                    model = Some(e.model.clone());
                }
                SessionEntry::ThinkingLevelChange(e) => {
                    thinking_level = e.thinking_level.clone();
                }
                SessionEntry::Label(e) => {
                    label = Some(e.label.clone());
                }
                SessionEntry::Leaf(e) => {
                    active_leaf_id = e.entry_id.clone();
                }
                SessionEntry::SessionInfo(e) => {
                    session_info = Some(e.clone());
                }
                SessionEntry::Custom(e) => {
                    custom_entries.push(e.clone());
                }
                SessionEntry::Compaction(e) => {
                    compaction_entries.push(e.clone());
                    message_rows = apply_compaction(message_rows, e);
                }
                SessionEntry::BranchSummary(e) => {
                    message_rows.push((e.id.clone(), format_branch_summary(e)));
                }
            }
        }

        Ok(SessionState {
            messages: message_rows.iter().map(|(_, m)| m.clone()).collect(),
            model,
            thinking_level,
            label,
            active_leaf_id,
            session_info,
            custom_entries,
            compaction_entries,
            context_entry_ids: message_rows.iter().map(|(id, _)| id.clone()).collect(),
            entries: replay.iter().map(|e| (*e).clone()).collect(),
        })
    }
}

fn apply_compaction(
    mut message_rows: Vec<(String, AgentMessage)>,
    entry: &CompactionEntry,
) -> Vec<(String, AgentMessage)> {
    let replaced_ids: std::collections::HashSet<String> =
        entry.replaces_entry_ids.iter().cloned().collect();
    let mut retained: Vec<(String, AgentMessage)> = Vec::new();
    let mut inserted_summary = false;
    for (id, msg) in message_rows.drain(..) {
        if !replaced_ids.contains(&id) {
            retained.push((id, msg));
            continue;
        }
        if !inserted_summary {
            retained.push((entry.id.clone(), format_compaction_summary(&entry.summary)));
            inserted_summary = true;
        }
    }
    if !inserted_summary {
        retained.push((entry.id.clone(), format_compaction_summary(&entry.summary)));
    }
    retained
}

fn format_compaction_summary(summary: &str) -> AgentMessage {
    AgentMessage::User(UserMessage::new(format!(
        "Previous conversation summary:\n{}",
        summary
    )))
}

fn format_branch_summary(entry: &BranchSummaryEntry) -> AgentMessage {
    AgentMessage::User(UserMessage::new(format!(
        "The following is a summary of a branch that this conversation came back from:\n<summary>\n{}\n</summary>",
        entry.summary
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::{MessageEntry, UserMessage};

    fn message_entry(msg: AgentMessage) -> MessageEntry {
        MessageEntry {
            id: tau_types::new_entry_id(),
            parent_id: None,
            timestamp: 0.0,
            r#type: tau_types::EntryType::Message,
            message: msg,
        }
    }

    #[test]
    fn linear_replay_keeps_order() {
        let entries: Vec<SessionEntry> = vec![
            SessionEntry::Message(Box::new(message_entry(AgentMessage::User(
                UserMessage::new("a"),
            )))),
            SessionEntry::Message(Box::new(message_entry(AgentMessage::User(
                UserMessage::new("b"),
            )))),
        ];
        let state = SessionState::from_entries(&entries, LeafSelector::Linear).unwrap();
        assert_eq!(state.messages.len(), 2);
    }

    #[test]
    fn at_none_yields_empty() {
        let entries: Vec<SessionEntry> = vec![SessionEntry::Message(Box::new(message_entry(
            AgentMessage::User(UserMessage::new("a")),
        )))];
        let state = SessionState::from_entries(&entries, LeafSelector::At(None)).unwrap();
        assert!(state.messages.is_empty());
    }
}
