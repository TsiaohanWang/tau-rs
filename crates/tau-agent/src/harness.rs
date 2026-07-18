//! Stateful reusable agent harness built on the portable loop.
//!
//! Mirrors `tau_agent.harness.AgentHarness` with an idiomatic Rust split:
//! shared interior-mutable state behind `Arc` so that `steer()` / `follow_up()`
//! / `cancel()` / `subscribe()` (all `&self`) remain callable while a prompt
//! stream is being driven — matching Python's single-object dual role as both
//! stream source and control panel. `prompt` returns a `Send + 'static` stream
//! that owns its own `Arc` handles and does not borrow the harness, so the
//! borrow checker enforces Python's "don't touch while running" discipline at
//! compile time (see ADR-3, ADR-4).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex, RwLock};

use async_stream::stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use tau_types::{AgentEvent, AgentMessage, TextContent, ToolResultMessage};

use crate::agent_loop::LoopArgs;
use crate::agent_loop::run_agent_loop;
use crate::provider::ModelProvider;
use crate::tool::{AfterToolCall, AgentTool, BeforeToolCall};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueMode {
    /// Drain one queued message per boundary (Python default).
    #[default]
    OneAtATime,
    /// Drain the entire queue at once.
    All,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueuedMessages {
    pub steering: Vec<AgentMessage>,
    pub follow_up: Vec<AgentMessage>,
}

impl QueuedMessages {
    pub fn count(&self) -> usize {
        self.steering.len() + self.follow_up.len()
    }
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.follow_up.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("AgentHarness is already running; use steer() or follow_up() to queue messages.")]
    AlreadyRunning,
}

/// Immutable harness configuration, stored as cheaply-clonable shared handles.
#[derive(Clone)]
pub struct AgentHarnessConfig {
    pub provider: Arc<dyn ModelProvider + Send + Sync>,
    pub model: String,
    pub system: String,
    pub tools: Vec<AgentTool>,
    pub max_turns: Option<u32>,
    pub queue_mode: QueueMode,
    pub before_tool_call: Option<Arc<dyn BeforeToolCall>>,
    pub after_tool_call: Option<Arc<dyn AfterToolCall>>,
}

impl AgentHarnessConfig {
    /// A minimal hobbiest-free builder: caller supplies provider/model/system;
    /// other fields default.
    pub fn new(
        provider: Arc<dyn ModelProvider + Send + Sync>,
        model: impl Into<String>,
        system: impl Into<String>,
    ) -> Self {
        AgentHarnessConfig {
            provider,
            model: model.into(),
            system: system.into(),
            tools: Vec::new(),
            max_turns: None,
            queue_mode: QueueMode::OneAtATime,
            before_tool_call: None,
            after_tool_call: None,
        }
    }
}

type ListenerEntry = (u64, Arc<dyn Fn(&AgentEvent) + Send + Sync>);

struct HarnessState {
    messages: Mutex<Vec<AgentMessage>>,
    steering: Mutex<VecDeque<AgentMessage>>,
    follow_up: Mutex<VecDeque<AgentMessage>>,
    signal: Mutex<Option<CancellationToken>>,
    replace_during_run: Mutex<Option<Vec<AgentMessage>>>,
    running: AtomicBool,
    listeners: RwLock<Vec<ListenerEntry>>,
    next_listener_id: AtomicU64,
}

impl HarnessState {
    fn notify(&self, event: &AgentEvent) {
        let listeners = self.listeners.read().unwrap();
        for (_id, listener) in listeners.iter() {
            listener(event);
        }
    }

    fn drain_queue(q: &Mutex<VecDeque<AgentMessage>>, mode: QueueMode) -> Vec<AgentMessage> {
        let mut q = q.lock().unwrap();
        match mode {
            QueueMode::All => q.drain(..).collect(),
            QueueMode::OneAtATime => q.pop_front().into_iter().collect(),
        }
    }
}

/// RAII owner of the taken-out messages; on drop it puts them back into shared
/// state (or honors a mid/end-run replacement), resets the running flag and
/// clears the signal. Runs on both normal completion and early stream drop.
struct MessagesGuard {
    state: Arc<HarnessState>,
    token: CancellationToken,
    messages: Option<Vec<AgentMessage>>,
}

impl Drop for MessagesGuard {
    fn drop(&mut self) {
        if let Some(mut msgs) = self.messages.take() {
            if self.token.is_cancelled() {
                append_interrupted_tool_results_on_vec(&mut msgs);
            }
            let mut shared = self.state.messages.lock().unwrap();
            if let Some(repl) = self.state.replace_during_run.lock().unwrap().take() {
                *shared = repl;
            } else {
                *shared = msgs;
            }
        }
        self.state.running.store(false, SeqCst);
        // Clear the signal if it hasn't been replaced by a new run.
        // Note: We can't compare CancellationToken values directly, so we just clear it.
        *self.state.signal.lock().unwrap() = None;
    }
}

pub struct AgentHarness {
    config_shared: HarnessConfigShared,
    state: Arc<HarnessState>,
}

#[derive(Clone)]
pub(crate) struct HarnessConfigShared {
    provider: Arc<dyn ModelProvider + Send + Sync>,
    model: String,
    system: String,
    tools: Arc<[AgentTool]>,
    max_turns: Option<u32>,
    queue_mode: QueueMode,
    before_tool_call: Option<Arc<dyn BeforeToolCall>>,
    after_tool_call: Option<Arc<dyn AfterToolCall>>,
}

impl AgentHarness {
    pub fn new(config: AgentHarnessConfig) -> Self {
        Self::with_messages(config, Vec::new())
    }

    pub fn with_messages(config: AgentHarnessConfig, messages: Vec<AgentMessage>) -> Self {
        let config_shared = HarnessConfigShared {
            provider: config.provider,
            model: config.model,
            system: config.system,
            tools: Arc::from(config.tools),
            max_turns: config.max_turns,
            queue_mode: config.queue_mode,
            before_tool_call: config.before_tool_call,
            after_tool_call: config.after_tool_call,
        };
        let state = Arc::new(HarnessState {
            messages: Mutex::new(messages),
            steering: Mutex::new(VecDeque::new()),
            follow_up: Mutex::new(VecDeque::new()),
            signal: Mutex::new(None),
            replace_during_run: Mutex::new(None),
            running: AtomicBool::new(false),
            listeners: RwLock::new(Vec::new()),
            next_listener_id: AtomicU64::new(1),
        });
        AgentHarness {
            config_shared,
            state,
        }
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.state.messages.lock().unwrap().clone()
    }

    pub fn is_running(&self) -> bool {
        self.state.running.load(SeqCst)
    }

    pub fn queued_messages(&self) -> QueuedMessages {
        QueuedMessages {
            steering: self
                .state
                .steering
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect(),
            follow_up: self
                .state
                .follow_up
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect(),
        }
    }

    pub fn pending_message_count(&self) -> usize {
        self.queued_messages().count()
    }

    pub fn has_queued_messages(&self) -> bool {
        !self.queued_messages().is_empty()
    }

    pub fn steer(&self, content: impl Into<String>) -> QueuedMessages {
        self.steer_message(AgentMessage::User(tau_types::UserMessage::new(
            tau_types::UserContent::text(content),
        )))
    }

    pub fn steer_message(&self, message: AgentMessage) -> QueuedMessages {
        self.state.steering.lock().unwrap().push_back(message);
        self.queued_messages()
    }

    pub fn follow_up(&self, content: impl Into<String>) -> QueuedMessages {
        self.follow_up_message(AgentMessage::User(tau_types::UserMessage::new(
            tau_types::UserContent::text(content),
        )))
    }

    pub fn follow_up_message(&self, message: AgentMessage) -> QueuedMessages {
        self.state.follow_up.lock().unwrap().push_back(message);
        self.queued_messages()
    }

    pub fn clear_queues(&self) -> QueuedMessages {
        let snapshot = self.queued_messages();
        self.state.steering.lock().unwrap().clear();
        self.state.follow_up.lock().unwrap().clear();
        snapshot
    }

    pub fn pop_latest_steering(&self) -> Option<AgentMessage> {
        self.state.steering.lock().unwrap().pop_back()
    }

    pub fn pop_latest_follow_up(&self) -> Option<AgentMessage> {
        self.state.follow_up.lock().unwrap().pop_back()
    }

    pub fn append_message(&self, message: AgentMessage) {
        let mut messages = self.state.messages.lock().unwrap();
        if self.state.running.load(SeqCst) {
            // Mid-run: defer by setting a replacement with the message appended.
            let pending = self
                .state
                .replace_during_run
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| messages.clone());
            let mut combined = pending;
            combined.push(message);
            *self.state.replace_during_run.lock().unwrap() = Some(combined);
        } else {
            messages.push(message);
        }
    }

    pub fn replace_messages(&self, messages: Vec<AgentMessage>) {
        if self.state.running.load(SeqCst) {
            *self.state.replace_during_run.lock().unwrap() = Some(messages);
        } else {
            *self.state.messages.lock().unwrap() = messages;
        }
    }

    pub fn cancel(&self) {
        if let Some(token) = self.state.signal.lock().unwrap().as_ref() {
            token.cancel();
        }
    }

    pub fn subscribe<F>(&self, listener: F) -> Unsubscribe
    where
        F: Fn(&AgentEvent) + Send + Sync + 'static,
    {
        let listener: Arc<dyn Fn(&AgentEvent) + Send + Sync> = Arc::new(listener);
        let id = self.state.next_listener_id.fetch_add(1, SeqCst);
        self.state.listeners.write().unwrap().push((id, listener));
        Unsubscribe {
            state: self.state.clone(),
            id,
        }
    }

    pub fn unsubscribe(&self, id: u64) {
        self.state
            .listeners
            .write()
            .unwrap()
            .retain(|(lid, _)| *lid != id);
    }

    pub fn append_interrupted_tool_results(&self) -> usize {
        let mut messages = self.state.messages.lock().unwrap();
        let before = messages.len();
        append_interrupted_tool_results_on_vec(&mut messages);
        messages.len() - before
    }

    pub fn prompt(
        &self,
        content: impl Into<String>,
    ) -> Result<impl futures::Stream<Item = AgentEvent> + Send + 'static, HarnessError> {
        self.prompt_message(AgentMessage::User(tau_types::UserMessage::new(
            tau_types::UserContent::text(content),
        )))
    }

    pub fn prompt_message(
        &self,
        message: AgentMessage,
    ) -> Result<impl futures::Stream<Item = AgentEvent> + Send + 'static, HarnessError> {
        self.start_run()?;
        self.append_interrupted_tool_results();
        let prompts = vec![message];
        Ok(self.drive_stream(prompts))
    }

    pub fn continue_(
        &self,
    ) -> Result<impl futures::Stream<Item = AgentEvent> + Send + 'static, HarnessError> {
        self.start_run()?;
        self.append_interrupted_tool_results();
        Ok(self.drive_stream(Vec::new()))
    }

    fn start_run(&self) -> Result<(), HarnessError> {
        match self
            .state
            .running
            .compare_exchange(false, true, SeqCst, SeqCst)
        {
            Ok(_) => {
                let token = CancellationToken::new();
                *self.state.signal.lock().unwrap() = Some(token);
                Ok(())
            }
            Err(_) => Err(HarnessError::AlreadyRunning),
        }
    }

    fn drive_stream(
        &self,
        prompts: Vec<AgentMessage>,
    ) -> impl futures::Stream<Item = AgentEvent> + Send + 'static {
        let state = self.state.clone();
        let config = self.config_shared.clone();
        let token = self
            .state
            .signal
            .lock()
            .unwrap()
            .as_ref()
            .cloned()
            .expect("signal set by start_run");
        stream! {
            let messages = std::mem::take(&mut *state.messages.lock().unwrap());
            // NOTE: append_interrupted_tool_results is NOT called here.
            // It was already called by prompt_message/continue_ before drive_stream
            // was invoked. Calling it again would be redundant (no-op in the common
            // case) or harmful on cancellation (MessagesGuard::drop handles the
            // post-cancel repair).  See docs/gap-analysis.md GAP-1.
            let queue_mode = config.queue_mode;
            let steering_state = state.clone();
            let follow_up_state = state.clone();
            let mut steering_drain = move || HarnessState::drain_queue(&steering_state.steering, queue_mode);
            let mut follow_up_drain = move || HarnessState::drain_queue(&follow_up_state.follow_up, queue_mode);

            let mut guard = MessagesGuard {
                state: state.clone(),
                token: token.clone(),
                messages: Some(messages),
            };
            let messages_ref = guard.messages.as_mut().expect("guard holds messages");
            let args = LoopArgs {
                provider: config.provider.as_ref(),
                model: &config.model,
                system: &config.system,
                messages: messages_ref,
                tools: &config.tools,
                prompts: &prompts,
                max_turns: config.max_turns,
                signal: Some(token.clone()),
                get_steering_messages: Some(&mut steering_drain),
                get_follow_up_messages: Some(&mut follow_up_drain),
                before_tool_call: config.before_tool_call.as_deref(),
                after_tool_call: config.after_tool_call.as_deref(),
            };
            let mut loop_stream = std::pin::pin!(run_agent_loop(args));
            while let Some(ev) = loop_stream.as_mut().next().await {
                state.notify(&ev);
                yield ev;
            }
            // `guard` drops on scope exit, putting messages back / resetting flags.
        }
    }
}

/// Handle returned by `subscribe`; calling `unsubscribe()` removes the listener.
pub struct Unsubscribe {
    state: Arc<HarnessState>,
    id: u64,
}

impl Unsubscribe {
    pub fn unsubscribe(self) {
        let mut listeners = self.state.listeners.write().unwrap();
        listeners.retain(|(lid, _)| *lid != self.id);
    }
}

/// Append synthetic `ToolResultMessage` error entries for any assistant
/// tool calls that never received a result. Mirrors Python's
/// `_append_interrupted_tool_results`.
pub(crate) fn append_interrupted_tool_results_on_vec(messages: &mut Vec<AgentMessage>) {
    let mut returned_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for message in messages.iter() {
        if let AgentMessage::ToolResult(tr) = message {
            returned_ids.insert(tr.tool_call_id.clone());
        }
    }
    // Collect dangling tool calls from assistant messages.
    let dangling: Vec<(String, String)> = {
        let mut out = Vec::new();
        for message in messages.iter() {
            if let AgentMessage::Assistant(assistant) = message {
                for call in assistant.tool_calls() {
                    if !returned_ids.contains(&call.id) {
                        returned_ids.insert(call.id.clone());
                        out.push((call.id.clone(), call.name.clone()));
                    }
                }
            }
        }
        out
    };
    for (id, name) in dangling {
        let tr = ToolResultMessage {
            role: tau_types::MessageRole::ToolResult,
            tool_call_id: id,
            tool_name: name,
            content: vec![tau_types::ToolResultContent::Text(TextContent::new(
                "Tool call interrupted by user",
            ))],
            details: value_object_empty(),
            added_tool_names: None,
            is_error: true,
            timestamp: tau_types::current_timestamp_ms(),
        };
        messages.push(AgentMessage::ToolResult(tr));
    }
}

fn value_object_empty() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}
