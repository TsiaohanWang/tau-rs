use std::sync::Arc;

use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::provider::ModelProvider;
use tau_types::{AgentMessage, SessionEntry};

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
/// directly.
pub struct CodingSession {
    storage: JsonlSessionStorage,
    harness: AgentHarness,
    last_entry_id: Option<String>,
    messages: Vec<AgentMessage>,
    entry_ids: Vec<String>,
    config: CodingSessionConfig,
}

impl CodingSession {
    pub fn new(storage: JsonlSessionStorage, config: CodingSessionConfig) -> Self {
        let tools = create_coding_tools(&config.cwd);
        let assembled_system = build_system_prompt(&tools, config.system.as_deref().unwrap_or(""));

        let harness = AgentHarness::new(AgentHarnessConfig {
            provider: config.provider.clone(),
            model: config.model.clone(),
            system: assembled_system,
            tools,
            max_turns: config.max_turns,
            queue_mode: QueueMode::OneAtATime,
            before_tool_call: None,
            after_tool_call: None,
        });

        Self {
            storage,
            harness,
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

    /// Send a user message, persist it, check context pressure, and trigger
    /// compaction if needed. Returns the harness stream.
    pub async fn prompt(
        &mut self,
        text: &str,
    ) -> Result<impl futures::Stream<Item = tau_types::AgentEvent> + '_, SessionError> {
        let user_msg = AgentMessage::User(tau_types::UserMessage::new(text));
        self.persist_with_parent(user_msg).await?;

        if let Some(window) = self.config.context_window {
            let estimate = estimate_context_usage(&self.messages, &[]);
            let reserve = self.config.compaction_reserve;
            if needs_compaction(&estimate, window, reserve) {
                let target = estimate.estimated_tokens.saturating_sub(window - reserve);
                if let Some(plan) = plan_compaction(&self.messages, &self.entry_ids, target) {
                    self.execute_compaction(plan).await?;
                }
            }
        }

        let stream = self.harness.prompt(text)?;
        Ok(stream)
    }

    /// Persist an assistant message from a harness event into the session.
    pub async fn persist_assistant(&mut self, message: AgentMessage) -> Result<(), SessionError> {
        self.persist_with_parent(message).await?;
        Ok(())
    }

    async fn persist_with_parent(&mut self, message: AgentMessage) -> Result<(), SessionError> {
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
        self.last_entry_id = Some(id);
        Ok(())
    }

    async fn execute_compaction(&mut self, plan: CompactionPlan) -> Result<(), SessionError> {
        let summary = self.generate_summary(&plan);
        let mut filled = plan;
        filled.summary = Some(summary);

        let compaction_entry = create_compaction_entry(&filled);
        self.storage
            .append(&SessionEntry::Compaction(compaction_entry))
            .await?;

        self.rebuild_after_compaction(&filled.entry_ids);
        Ok(())
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
        }
    }
}
