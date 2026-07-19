use std::sync::Arc;

use async_stream::stream;
use futures::stream::StreamExt;
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::provider::ModelProvider;
use tau_agent::tool::AgentTool;
use tau_types::{AgentEvent, AgentMessage, SessionEntry};

use crate::prompt::build_system_prompt;
use crate::session::compaction::{CompactionPlan, create_compaction_entry, plan_compaction};
use crate::session::context_window::{estimate_context_usage, needs_compaction};
use crate::session::storage::{JsonlSessionStorage, SessionError};
use crate::tools::create_coding_tools;

/// Configuration for a `CodingSession`.
pub struct CodingSessionConfig {
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    pub model: String,
    pub system: Option<String>,
    pub cwd: std::path::PathBuf,
    pub max_turns: Option<u32>,
    pub context_window: Option<u64>,
    pub compaction_reserve: u64,
}

/// The composition root for a coding session.
///
/// Owns persistence, tools, system-prompt assembly, context-window estimation,
/// and compaction. The CLI delegates to this instead of driving the harness
/// directly — see ADR-P5-1 / ADR-P5-2 in `docs/phase-5.md`.
///
/// Persistence is an *invariant side effect* of consuming the stream returned
/// by [`CodingSession::prompt`]: each `MessageEnd` event is auto-persisted as
/// a `MessageEntry` chained off the previous persisted entry via `parent_id`,
/// plus a `LeafEntry` pointing at the latest message. The caller does not
/// need to call any persist function. See ADR-P5-2.
pub struct CodingSession {
    storage: JsonlSessionStorage,
    harness: AgentHarness,
    tools: Arc<[AgentTool]>,
    last_entry_id: Option<String>,
    /// In-memory mirror of persisted messages, parallel to what the harness
    /// owns. Used for compaction planning + post-compaction rebuild. Kept in
    /// sync with the harness via [`rebuild_after_compaction`].
    messages: Vec<AgentMessage>,
    entry_ids: Vec<String>,
    config: CodingSessionConfig,
}

impl CodingSession {
    /// Create a brand-new session with empty history.
    pub fn new(storage: JsonlSessionStorage, config: CodingSessionConfig) -> Self {
        let tools: Arc<[AgentTool]> = Arc::from(create_coding_tools(&config.cwd));
        let assembled_system = build_system_prompt(&tools, config.system.as_deref().unwrap_or(""));

        let harness = AgentHarness::new(AgentHarnessConfig {
            provider: config.provider.clone(),
            model: config.model.clone(),
            system: assembled_system,
            tools: tools.to_vec(),
            max_turns: config.max_turns,
            queue_mode: QueueMode::OneAtATime,
            before_tool_call: None,
            after_tool_call: None,
        });

        Self {
            storage,
            harness,
            tools,
            last_entry_id: None,
            messages: Vec::new(),
            entry_ids: Vec::new(),
            config,
        }
    }

    pub fn storage(&self) -> &JsonlSessionStorage {
        &self.storage
    }

    pub fn harness(&self) -> &AgentHarness {
        &self.harness
    }

    /// Tools owned by this session (read-only view for renderers).
    pub fn tools(&self) -> &[AgentTool] {
        &self.tools
    }

    /// Current in-memory message log (a snapshot of what the harness holds).
    pub fn messages(&self) -> Vec<AgentMessage> {
        self.harness.messages()
    }

    /// The model the harness is currently configured for.
    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Append the `SessionInfo` entry to a brand-new session file.
    ///
    /// Should be called once immediately after [`CodingSession::new`] for fresh
    /// sessions (in 5.2, [`CodingSession::load`] will already have the row).
    pub async fn write_session_info(&mut self) -> Result<(), SessionError> {
        let info = SessionEntry::SessionInfo(tau_types::SessionInfoEntry {
            id: tau_types::message::new_entry_id(),
            parent_id: None,
            timestamp: tau_types::current_timestamp_secs(),
            r#type: tau_types::EntryType::SessionInfo,
            created_at: tau_types::current_timestamp_secs(),
            cwd: self.config.cwd.to_str().map(|s| s.to_string()),
            title: None,
        });
        self.storage.append(&info).await
    }

    /// Send a user message and return a stream that:
    /// 1. runs the pre-prompt compaction threshold check (5.3 will lift the
    ///    summary LLM call here; for now the summary is a debug placeholder);
    /// 2. drives the harness stream;
    /// 3. auto-persists every `MessageEnd` event — both the user prompt echo
    ///    (the agent loop yields `MessageEnd` once per `prompt`) and the
    ///    assistant reply — chained off the previous persisted entry via
    ///    `parent_id`.
    ///
    /// The returned stream borrows `&mut self` for its lifetime — only one
    /// prompt can be in flight at a time, enforced at compile time.
    ///
    /// Harness start errors (which structurally cannot occur here because we
    /// hold `&mut self` — there is no way for two run streams to co-exist)
    /// silently truncate the stream rather than panic.
    pub fn prompt<'a>(
        &'a mut self,
        text: &'a str,
    ) -> Result<impl futures::Stream<Item = AgentEvent> + Send + 'a, SessionError> {
        Ok(stream! {
            // 1. Pre-prompt compaction threshold check (ADR-P5-3 §5.3).
            if let Some(window) = self.config.context_window {
                let estimate = estimate_context_usage(&self.messages, &[]);
                let reserve = self.config.compaction_reserve;
                if needs_compaction(&estimate, window, reserve) {
                    let target = estimate.estimated_tokens.saturating_sub(window - reserve);
                    if let Some(plan) = plan_compaction(&self.messages, &self.entry_ids, target) {
                        let _ = self.execute_compaction(plan).await;
                    }
                }
            }

            // 2. Start the harness run. `&mut self` forbids two concurrent
            //    streams, so `Err` here is a logic bug — fail closed by
            //    returning early (caller's consumer loop simply ends).
            let inner = match self.harness.prompt(text) {
                Ok(s) => s,
                Err(_) => return,
            };

            // 3. Drive the harness. The agent loop yields one `MessageEnd` per
            //    message it sees — including the user prompt it just loaded —
            //    so this persistence hook covers user + assistant uniformly.
            //    `persist_with_parent` advances `last_entry_id`, giving every
            //    subsequent entry the correct parent.
            futures::pin_mut!(inner);
            while let Some(ev) = inner.next().await {
                if let AgentEvent::MessageEnd(ref end) = ev {
                    let _ = self.persist_with_parent(end.message.clone()).await;
                }
                yield ev;
            }
        })
    }

    async fn persist_with_parent(&mut self, message: AgentMessage) -> Result<String, SessionError> {
        let id = tau_types::message::new_entry_id();

        let entry = SessionEntry::Message(Box::new(tau_types::MessageEntry {
            id: id.clone(),
            parent_id: self.last_entry_id.clone(),
            timestamp: tau_types::current_timestamp_secs(),
            r#type: tau_types::EntryType::Message,
            message: message.clone(),
        }));
        self.storage.append(&entry).await?;

        let leaf = SessionEntry::Leaf(tau_types::LeafEntry {
            id: tau_types::message::new_entry_id(),
            parent_id: None,
            timestamp: tau_types::current_timestamp_secs(),
            r#type: tau_types::EntryType::Leaf,
            entry_id: Some(id.clone()),
        });
        self.storage.append(&leaf).await?;

        self.messages.push(message);
        self.entry_ids.push(id.clone());
        self.last_entry_id = Some(id.clone());
        Ok(id)
    }

    async fn execute_compaction(
        &mut self,
        plan: CompactionPlan,
    ) -> Result<Option<String>, SessionError> {
        let summary = self.generate_summary(&plan);
        let mut filled = plan;
        filled.summary = Some(summary);

        let compaction_entry = create_compaction_entry(&filled);
        let compaction_id = compaction_entry.id.clone();
        let compacted_ids = filled.entry_ids.clone();

        let entry = SessionEntry::Compaction(compaction_entry);
        self.storage.append(&entry).await?;

        self.rebuild_after_compaction(&compacted_ids);
        Ok(Some(compaction_id))
    }

    fn generate_summary(&self, plan: &CompactionPlan) -> String {
        let parts: Vec<String> = self
            .messages
            .iter()
            .zip(self.entry_ids.iter())
            .filter(|(_, id)| plan.entry_ids.contains(id))
            .map(|(m, _)| format!("{m:?}"))
            .collect();

        let total = parts.len();
        let combined = parts.join("\n");
        let preview = if combined.len() > 2000 {
            &combined[..2000]
        } else {
            &combined
        };
        format!("[Compacted {total} messages] {preview}")
    }

    fn rebuild_after_compaction(&mut self, compacted_ids: &[String]) {
        let last_idx = self
            .entry_ids
            .iter()
            .rposition(|id| compacted_ids.contains(id));

        if let Some(idx) = last_idx {
            let remaining_msgs: Vec<AgentMessage> = self.messages.split_off(idx + 1);
            let remaining_ids: Vec<String> = self.entry_ids.split_off(idx + 1);

            let summary = AgentMessage::User(tau_types::UserMessage::new(
                "[Context compacted. Earlier messages summarized.]",
            ));
            self.messages = vec![summary];
            self.messages.extend(remaining_msgs);
            self.entry_ids = vec!["compacted".to_string()];
            self.entry_ids.extend(remaining_ids);

            // Push the rebuilt list into the harness so the next prompt starts
            // from post-compaction state. ADR-P5-3 §3.3 step 3 — full wiring of
            // `replace_messages` lands in 5.3 (currently the harness keeps its
            // own parallel list; 5.3 will canonicalize on `self.messages`).
            self.harness.replace_messages(self.messages.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::{AgentMessage, UserMessage};
    use tempfile::TempDir;

    struct DummyProvider;
    impl ModelProvider for DummyProvider {
        fn stream_response<'a>(
            &'a self,
            _request: &'a tau_agent::provider::StreamRequest<'a>,
        ) -> futures::stream::BoxStream<'a, tau_types::AssistantMessageEvent> {
            futures::stream::iter(Vec::new()).boxed()
        }
    }

    fn make_session(dir: &TempDir) -> CodingSession {
        let storage = JsonlSessionStorage::new(dir.path().join("s.jsonl"));
        CodingSession::new(
            storage,
            CodingSessionConfig {
                provider: Arc::new(DummyProvider) as _,
                model: "test-model".into(),
                system: None,
                cwd: dir.path().to_path_buf(),
                max_turns: Some(4),
                context_window: None,
                compaction_reserve: 16384,
            },
        )
    }

    #[tokio::test]
    async fn persist_chains_parent_ids() {
        let dir = TempDir::new().unwrap();
        let mut s = make_session(&dir);
        let id1 = s
            .persist_with_parent(AgentMessage::User(UserMessage::new("first")))
            .await
            .unwrap();
        let id2 = s
            .persist_with_parent(AgentMessage::User(UserMessage::new("second")))
            .await
            .unwrap();
        assert_ne!(id1, id2);
        assert_eq!(s.last_entry_id.as_deref(), Some(id2.as_str()));

        let entries = s.storage().read_all().await.unwrap();
        // 2 × (Message + Leaf) = 4 entries
        assert_eq!(entries.len(), 4);

        // The second MessageEntry's parent_id points at the first message id.
        let second_msg = entries
            .iter()
            .find_map(|e| match e {
                SessionEntry::Message(m) if m.id == id2 => Some(m),
                _ => None,
            })
            .unwrap();
        assert_eq!(second_msg.parent_id.as_deref(), Some(id1.as_str()));
    }

    #[tokio::test]
    async fn write_session_info_appends_row() {
        let dir = TempDir::new().unwrap();
        let mut s = make_session(&dir);
        s.write_session_info().await.unwrap();
        let entries = s.storage().read_all().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0], SessionEntry::SessionInfo(_)));
    }
}
