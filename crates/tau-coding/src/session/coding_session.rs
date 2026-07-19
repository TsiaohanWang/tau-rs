use std::sync::Arc;

use async_stream::stream;
use futures::stream::StreamExt;
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::provider::{ModelProvider, StreamRequest};
use tau_agent::session::state::{LeafSelector, SessionState};
use tau_agent::tool::AgentTool;
use tau_types::{AgentEvent, AgentMessage, SessionEntry};

use crate::prompt::build_system_prompt;
use crate::session::compaction::{CompactionPlan, create_compaction_entry, plan_compaction};
use crate::session::compaction::{build_compaction_summary_prompt, summarization_system_prompt};
use crate::session::context_window::{estimate_context_usage, needs_compaction};
use crate::session::repair::repair_interrupted_tool_calls;
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
    /// Requested provider name (in-memory only; set via `/provider`). Not used
    /// to rebuild credentials in Phase 5.
    pub provider_name: Option<String>,
    /// Active thinking/reasoning-effort level. `None` uses the provider
    /// default. Set via `/thinking` / `set_thinking_level`.
    pub thinking_level: Option<String>,
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
    /// Guard flag to prevent compaction-retry loops: when an overflow error
    /// triggers a compaction + retry, this is set to `true` so that if the
    /// retried call also fails we do not compact again.
    is_retrying_compaction: bool,
    /// Auto-derived session title (from the first user prompt). `None` until
    /// the first `prompt` call persists a `LabelEntry`. Set on `load` from the
    /// session's `SessionInfo.title` when present.
    title: Option<String>,
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
            thinking_level: config.thinking_level.clone(),
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
            is_retrying_compaction: false,
            title: None,
        }
    }

    /// Load an existing session from JSONL: read every entry, replay via
    /// `SessionState::from_entries` (applying any compaction summaries), repair
    /// any interrupted tool calls, then build a `CodingSession` whose harness
    /// sees the reconstructed conversation so the next prompt continues it
    /// rather than starting over.
    ///
    /// `last_entry_id` is initialized from the session's latest `LeafEntry`,
    /// so subsequent `prompt` calls chain their new `MessageEntry` off the
    /// persisted tail instead of creating orphan roots. See ADR-P5-4.
    pub async fn load(
        storage: JsonlSessionStorage,
        config: CodingSessionConfig,
    ) -> Result<Self, SessionError> {
        let entries = storage.read_all().await?;
        let state = SessionState::from_entries(&entries, LeafSelector::Linear).map_err(|e| {
            SessionError::Jsonl(tau_agent::session::jsonl::SessionJsonlError {
                line_number: None,
                message: format!("session tree walk failed: {e}"),
            })
        })?;

        // Reconstruct the in-memory message chain + parallel entry_ids,
        // then repair any interrupted tool calls in memory.
        let mut messages = state.messages.clone();
        let mut entry_ids = state.context_entry_ids.clone();
        let _ = repair_interrupted_tool_calls(&mut messages);
        // The repair may have inserted synthetic tool_results that don't have
        // corresponding entry_ids in the JSONL. Mirror them with placeholder
        // "rebuilt-..." ids so the parallel-vec invariant holds (these are
        // in-memory-only and never persisted).
        while entry_ids.len() < messages.len() {
            entry_ids.push(format!("rebuilt-{}", entry_ids.len()));
        }

        let tools: Arc<[AgentTool]> = Arc::from(create_coding_tools(&config.cwd));
        let assembled_system = build_system_prompt(&tools, config.system.as_deref().unwrap_or(""));

        let harness = AgentHarness::with_messages(
            AgentHarnessConfig {
                provider: config.provider.clone(),
                model: config.model.clone(),
                system: assembled_system,
                tools: tools.to_vec(),
                max_turns: config.max_turns,
                queue_mode: QueueMode::OneAtATime,
                thinking_level: config.thinking_level.clone(),
                before_tool_call: None,
                after_tool_call: None,
            },
            messages.clone(),
        );

        // Restore the session title from the persisted `SessionInfo`, if any.
        // This keeps a resumed session from re-deriving a fresh title on the
        // next `prompt` call (which would append a duplicate `LabelEntry`).
        let title = entries.iter().find_map(|e| match e {
            SessionEntry::SessionInfo(info) => info.title.clone(),
            _ => None,
        });

        Ok(Self {
            storage,
            harness,
            tools,
            last_entry_id: state.active_leaf_id,
            messages,
            entry_ids,
            config,
            is_retrying_compaction: false,
            title,
        })
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

    /// The auto-derived (or loaded) session title, if any.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Switch the active model (in-memory only — Phase 5 does not persist the
    /// change to the journal). The harness is rebuilt so subsequent prompts use
    /// the new model.
    pub fn set_model(&mut self, model: String) {
        self.config.model = model.clone();
        self.harness.set_model(model);
    }

    /// Record the requested provider name in memory. Phase 5 does not persist
    /// the change or rebuild credentials for an arbitrary provider, so this is
    /// a display-only switch (`model()`-style naming is deferred to a later
    /// phase that wires credential resolution into the session).
    pub fn set_provider(&mut self, provider: String) {
        self.config.provider_name = Some(provider);
    }

    /// Set the thinking/reasoning-effort level (in-memory). `None` reverts to
    /// the provider default. Applies to the next `prompt` via the harness.
    pub fn set_thinking_level(&mut self, level: Option<String>) {
        self.config.thinking_level = level.clone();
        self.harness.set_thinking_level(level);
    }

    /// The current thinking level, if set.
    pub fn thinking_level(&self) -> Option<&str> {
        self.config.thinking_level.as_deref()
    }

    /// Drop all in-memory messages. The persisted journal is left untouched;
    /// subsequent prompts start a fresh turn that chains off the last leaf.
    pub fn clear_messages(&mut self) {
        self.harness.clear_messages();
        self.messages.clear();
        self.entry_ids.clear();
    }

    /// Force a context compaction now (used by the `/compact` command).
    /// Returns `Ok(true)` if a compaction was performed, `Ok(false)` if there
    /// was nothing to compact.
    pub async fn compact_now(&mut self) -> Result<bool, SessionError> {
        if self.config.context_window.is_none() {
            return Ok(false);
        }
        let window = self.config.context_window.unwrap();
        let reserve = self.config.compaction_reserve;
        let estimate = estimate_context_usage(&self.messages, &[]);
        if !needs_compaction(&estimate, window, reserve) {
            return Ok(false);
        }
        let target = estimate.estimated_tokens.saturating_sub(window - reserve);
        match plan_compaction(&self.messages, &self.entry_ids, target) {
            Some(plan) => {
                self.execute_compaction(plan).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Persist an auto-derived title as a `LabelEntry`, chaining off the last
    /// persisted entry. No-op if a title has already been set.
    async fn ensure_title(&mut self, first_user: &str) -> Result<(), SessionError> {
        if self.title.is_some() {
            return Ok(());
        }
        let title = crate::naming::auto_title(first_user, &self.config.cwd);
        let id = tau_types::message::new_entry_id();
        let entry = SessionEntry::Label(tau_types::LabelEntry {
            id: id.clone(),
            parent_id: self.last_entry_id.clone(),
            timestamp: tau_types::current_timestamp_secs(),
            r#type: tau_types::EntryType::Label,
            label: title.clone(),
        });
        self.storage.append(&entry).await?;
        self.last_entry_id = Some(id);
        self.title = Some(title);
        Ok(())
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
    /// 1. runs the pre-prompt compaction threshold check (5.3); on overflow
    ///    error it triggers one-shot compaction + retry (see `is_overflow_error`);
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
            // 0. Derive + persist a session title from the first user prompt
            // (ADR-P5-4 auto-naming). Chains off the last persisted entry and
            // runs only once per session.
            if self.title.is_none() {
                let _ = self.ensure_title(text).await;
            }

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

            // 2. Start the harness run.
            let inner = match self.harness.prompt(text) {
                Ok(s) => s,
                Err(_) => return,
            };

            // 3. Drive the harness, watching for context-overflow errors.
            futures::pin_mut!(inner);
            while let Some(ev) = inner.next().await {
                // Check for overflow error on MessageEnd → trigger one-shot
                // compaction + retry (ADR-P5-3 overflow path).
                if let AgentEvent::MessageEnd(ref end) = ev {
                    if !self.is_retrying_compaction && is_overflow_error(&end.message) {
                        self.is_retrying_compaction = true;
                        // Plan compaction: free enough tokens to continue.
                        let estimate = estimate_context_usage(&self.messages, &[]);
                        let window = self.config.context_window.unwrap_or(128_000);
                        let reserve = self.config.compaction_reserve;

                        let target = estimate.estimated_tokens.saturating_sub(window - reserve);
                        if let Some(plan) = plan_compaction(&self.messages, &self.entry_ids, target) {
                            let _ = self.execute_compaction(plan).await;
                        }
                        // Yield the error event first so the caller sees it,
                        // then re-drive from the same prompt text.
                        yield ev;
                        // Drain the rest of the current harness stream so its
                        // RAII `MessagesGuard` drops and releases the harness's
                        // `running` lock before we start the retry run.
                        while inner.next().await.is_some() {}
                        let retry_inner = match self.harness.prompt(text) {
                            Ok(s) => s,
                            Err(_) => return,
                        };
                        futures::pin_mut!(retry_inner);
                        while let Some(retry_ev) = retry_inner.next().await {
                            if let AgentEvent::MessageEnd(ref retry_end) = retry_ev {
                                let _ = self.persist_with_parent(retry_end.message.clone()).await;
                            }
                            yield retry_ev;
                        }
                        return;
                    }
                }
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
        let summary = self.generate_summary(&plan).await;
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

    /// Generate a summary of the messages targeted by the compaction plan.
    ///
    /// Builds a single-message prompt and calls the provider directly for
    /// summarization (not via the harness, which would add tools + system).
    /// Falls back to a deterministic debug-format string if the LLM call fails
    /// (e.g. network error, rate limit, model refusal).
    async fn generate_summary(&self, plan: &CompactionPlan) -> String {
        let user_prompt = build_compaction_summary_prompt(&self.messages);

        let user_message = AgentMessage::User(tau_types::UserMessage::new(user_prompt.as_str()));
        let request = StreamRequest {
            model: &self.config.model,
            system: summarization_system_prompt(),
            messages: &[user_message],
            tools: &[],
            signal: None,
            thinking_level: None,
        };

        let mut stream = self.config.provider.stream_response(&request);
        let mut text = String::new();

        while let Some(event) = stream.next().await {
            if let tau_types::AssistantMessageEvent::TextDelta(delta) = event {
                text.push_str(&delta.delta);
            }
            // We ignore other event types (ToolCall, Done, etc.) — the
            // summarization prompt asks for text-only output.
        }

        if text.trim().is_empty() {
            eprintln!(
                "warning: LLM summarization returned empty text; falling back to debug summary"
            );
            self.generate_debug_summary(plan)
        } else {
            text.trim().to_string()
        }
    }

    /// Deterministic fallback summary when the LLM call fails or returns empty.
    fn generate_debug_summary(&self, plan: &CompactionPlan) -> String {
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
            // from post-compaction state. ADR-P5-3 §3.3 step 3 — the harness
            // now canonicalizes on `self.messages` after compaction.
            self.harness.replace_messages(self.messages.clone());
        }
    }
}

/// Detect a provider context-overflow error in a final assistant message.
///
/// Overflow errors surface as `stop_reason = Error` with keywords like
/// "context length", "token limit", or "context window" in the text. Used to
/// trigger a one-shot compaction + retry.
fn is_overflow_error(message: &AgentMessage) -> bool {
    let is_error_stop = matches!(
        message,
        AgentMessage::Assistant(a) if a.stop_reason == tau_types::StopReason::Error
    );
    if !is_error_stop {
        return false;
    }
    let text = message.text().to_ascii_lowercase();
    text.contains("context length")
        || text.contains("context window")
        || text.contains("token limit")
        || text.contains("too long")
        || text.contains("maximum context")
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
                provider_name: None,
                thinking_level: None,
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

    // --- Phase 5.3: compaction / summarization tests -----------------------

    /// Build a session backed by a `FakeProvider` that serves the given
    /// event batches per `stream_response` call (in order).
    fn make_session_with_provider(
        dir: &TempDir,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        context_window: Option<u64>,
    ) -> CodingSession {
        let storage = JsonlSessionStorage::new(dir.path().join("s.jsonl"));
        CodingSession::new(
            storage,
            CodingSessionConfig {
                provider,
                model: "test-model".into(),
                system: None,
                cwd: dir.path().to_path_buf(),
                max_turns: Some(20),
                context_window,
                compaction_reserve: 16384,
                provider_name: None,
                thinking_level: None,
            },
        )
    }

    async fn assistant_with_text(text: &str) -> tau_types::AssistantMessage {
        let mut a = tau_types::AssistantMessage::default();
        a.content.push(tau_types::AssistantContent::Text(
            tau_types::TextContent::new(text),
        ));
        a.stop_reason = tau_types::StopReason::Stop;
        a
    }

    fn overflow_error_message() -> tau_types::AssistantMessage {
        let mut a = tau_types::AssistantMessage::default();
        a.content.push(tau_types::AssistantContent::Text(
            tau_types::TextContent::new("maximum context length exceeded"),
        ));
        a.stop_reason = tau_types::StopReason::Error;
        a.error_message = Some("maximum context length exceeded".to_string());
        a
    }

    #[tokio::test]
    async fn generate_summary_uses_llm_provider() {
        use tau_agent::testing::{assistant_done, assistant_start, text_delta};

        let dir = TempDir::new().unwrap();
        // Provider serves a single batch: a summary text + done.
        let provider = Arc::new(tau_agent::testing::FakeProvider::with_events(vec![
            assistant_start(None, None),
            text_delta("## Goal\nBuild the parser.\n## Progress\n- [x] scaffold"),
            assistant_done(assistant_with_text("## Goal\nBuild the parser.").await),
        ]));
        let mut session = make_session_with_provider(&dir, provider, None);

        // Seed a couple of messages so the summary prompt has content.
        session
            .persist_with_parent(AgentMessage::User(UserMessage::new("build a parser")))
            .await
            .unwrap();
        session
            .persist_with_parent(AgentMessage::Assistant(assistant_with_text("ok").await))
            .await
            .unwrap();

        let plan = CompactionPlan {
            entry_ids: session.entry_ids.clone(),
            summary: None,
        };
        let summary = session.generate_summary(&plan).await;
        assert!(
            summary.contains("Build the parser"),
            "summary should reflect the LLM output, got: {summary}"
        );
    }

    #[tokio::test]
    async fn generate_summary_falls_back_when_llm_empty() {
        use tau_agent::testing::{assistant_done, assistant_start};

        let dir = TempDir::new().unwrap();
        // Provider returns Done with no text deltas → empty summary → fallback.
        let provider = Arc::new(tau_agent::testing::FakeProvider::with_events(vec![
            assistant_start(None, None),
            assistant_done(assistant_with_text("").await),
        ]));
        let mut session = make_session_with_provider(&dir, provider, None);
        session
            .persist_with_parent(AgentMessage::User(UserMessage::new("hi")))
            .await
            .unwrap();

        let plan = CompactionPlan {
            entry_ids: session.entry_ids.clone(),
            summary: None,
        };
        let summary = session.generate_summary(&plan).await;
        assert!(
            summary.starts_with("[Compacted"),
            "empty LLM summary must fall back to debug format, got: {summary}"
        );
    }

    #[tokio::test]
    async fn overflow_retry_compacts_and_retries_once() {
        use tau_agent::testing::{assistant_done, assistant_start, text_delta};

        let dir = TempDir::new().unwrap();
        // Batch order:
        //   0: first harness prompt → overflow error
        //   1: compaction summarization call → summary text
        //   2: retry harness prompt → success
        let provider = Arc::new(tau_agent::testing::FakeProvider::new(vec![
            vec![
                assistant_start(None, None),
                assistant_done(overflow_error_message()),
            ],
            vec![
                assistant_start(None, None),
                text_delta("## Goal\nDone."),
                assistant_done(assistant_with_text("## Goal\nDone.").await),
            ],
            vec![
                assistant_start(None, None),
                text_delta("retry answer"),
                assistant_done(assistant_with_text("retry answer").await),
            ],
        ]));

        let mut session = make_session_with_provider(&dir, provider.clone(), None);

        // Seed enough history that the threshold check could trigger, but the
        // key assertion is the overflow retry path.
        session
            .persist_with_parent(AgentMessage::User(UserMessage::new("first")))
            .await
            .unwrap();

        let stream = session.prompt("overflow now").unwrap();
        futures::pin_mut!(stream);
        let mut saw_assistant = false;
        while let Some(ev) = stream.next().await {
            if let tau_types::AgentEvent::MessageEnd(end) = ev {
                if matches!(end.message, AgentMessage::Assistant(_)) {
                    saw_assistant = true;
                }
            }
        }

        assert!(
            saw_assistant,
            "retry should yield a successful assistant turn"
        );
        // 3 provider calls: original, compaction, retry.
        assert_eq!(
            provider.call_count(),
            3,
            "overflow must trigger exactly one compaction + one retry"
        );
    }

    #[tokio::test]
    async fn execute_compaction_reduces_harness_messages() {
        use tau_agent::testing::{assistant_done, assistant_start, text_delta};

        let dir = TempDir::new().unwrap();
        let provider = Arc::new(tau_agent::testing::FakeProvider::new(vec![vec![
            assistant_start(None, None),
            text_delta("summary text"),
            assistant_done(assistant_with_text("summary text").await),
        ]]));

        let mut session = make_session_with_provider(&dir, provider, None);
        // 4 messages: user, assistant, user, assistant (each long enough to
        // estimate > 0 tokens so the compaction plan can free multiple).
        for i in 0..2 {
            let long = format!("message number {i} with enough content to exceed four chars");
            session
                .persist_with_parent(AgentMessage::User(UserMessage::new(long.clone())))
                .await
                .unwrap();
            session
                .persist_with_parent(AgentMessage::Assistant(assistant_with_text(&long).await))
                .await
                .unwrap();
        }
        let before = session.messages.len();
        assert_eq!(before, 4);

        // Target 20 tokens: with ~13 tokens per message this compacts the
        // first two messages (26 >= 20), leaving [summary] + 2 = 3.
        let plan = plan_compaction(&session.messages, &session.entry_ids, 20).unwrap();
        assert!(
            plan.entry_ids.len() >= 2,
            "plan should compact multiple messages"
        );
        session.execute_compaction(plan).await.unwrap();

        let after = session.messages.len();
        assert!(
            after < before,
            "compaction should reduce message count: {before} → {after}"
        );
        // After compaction the first message is the summary placeholder.
        assert!(
            matches!(session.messages.first(), Some(AgentMessage::User(_))),
            "first message after compaction should be the summary"
        );
    }
}
