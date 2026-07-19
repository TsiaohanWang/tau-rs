//! Translate Pi-compatible `AgentEvent`s into [`TuiState`] (mirrors original
//! `tui/adapter.py::TuiEventAdapter.apply`). Pure logic — no ratatui/terminal
//! dependency, so it is unit-testable without a TTY.

use tau_types::{AgentEvent, AgentMessage, CustomMessage, MessageStartEvent};

use super::state::{ChatItemRole, TuiState};

/// Incremental projector of agent events into display state. Holds the live
/// [`TuiState`] and mutates it in place per event (same ownership model as the
/// original `self.state = state`).
#[derive(Default)]
pub struct TuiEventAdapter {
    state: TuiState,
}

impl TuiEventAdapter {
    #[allow(dead_code)]
    pub fn new() -> Self {
        TuiEventAdapter {
            state: TuiState::new(),
        }
    }

    pub fn state(&self) -> &TuiState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut TuiState {
        &mut self.state
    }

    /// Apply one agent event, mutating the held [`TuiState`]. Mirrors the
    /// branch order of `adapter.py::apply`.
    pub fn apply(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::AgentStart(_) => {
                self.state.running = true;
                self.state.error = None;
            }
            AgentEvent::AgentEnd(_) => {
                self.flush();
                self.state.running = false;
            }
            AgentEvent::MessageStart(MessageStartEvent { message }) => {
                if let AgentMessage::Assistant(_) = message {
                    self.state.assistant_buffer.clear();
                    self.state.begin_assistant();
                }
            }
            AgentEvent::MessageUpdate(update) => match &update.assistant_message_event {
                tau_types::AssistantMessageEvent::TextDelta(d) => {
                    self.state.assistant_buffer.push_str(&d.delta);
                }
                tau_types::AssistantMessageEvent::ThinkingDelta(d) => {
                    self.state.add_thinking_delta(&d.delta);
                }
                _ => {}
            },
            AgentEvent::MessageEnd(end) => {
                let message = &end.message;
                match message {
                    AgentMessage::User(u) => self.state.add_user_message(&u.text(), None, None),
                    AgentMessage::Custom(c) => self.apply_custom(c),
                    AgentMessage::Assistant(a) => self.state.finalize_assistant(a),
                    _ => {}
                }
            }
            AgentEvent::ToolExecutionStart(start) => {
                self.flush();
                let call = tau_types::ToolCall {
                    r#type: tau_types::ContentBlockType::ToolCall,
                    id: start.tool_call_id.clone(),
                    name: start.tool_name.clone(),
                    arguments: start.args.clone(),
                    thought_signature: None,
                };
                self.state.add_tool_call(&call);
            }
            AgentEvent::ToolExecutionUpdate(update) => {
                let text = update.partial_result.text();
                self.state.record_tool_update(&update.tool_call_id, &text);
            }
            AgentEvent::ToolExecutionEnd(end) => {
                let text = end.result.text();
                self.state.record_tool_result(
                    &end.tool_call_id,
                    &end.tool_name,
                    &text,
                    end.is_error,
                );
            }
            // TurnStart / TurnEnd carry no transcript content in the original
            // adapter; they are no-ops for display state.
            AgentEvent::TurnStart(_) | AgentEvent::TurnEnd(_) => {}
        }
    }

    fn apply_custom(&mut self, c: &CustomMessage) {
        self.state.add_user_message(
            &c.text(),
            Some(c.custom_type.clone()),
            if c.details.is_object() || c.details.is_array() {
                Some(c.details.clone())
            } else {
                None
            },
        );
    }

    fn flush(&mut self) {
        if !self.state.assistant_buffer.is_empty() {
            let text = std::mem::take(&mut self.state.assistant_buffer);
            self.state.add_item(ChatItemRole::Assistant, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tau_types::{
        AgentEndEvent, AgentEvent, AgentMessage, AgentStartEvent, AssistantContent,
        AssistantMessage, AssistantMessageEvent, MessageEndEvent, MessageStartEvent, TextContent,
        TextDeltaEvent, ThinkingDeltaEvent, ToolExecutionEndEvent, ToolExecutionStartEvent,
        UserMessage,
    };

    fn assistant_with_text(text: &str) -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text(TextContent::new(text))],
            ..Default::default()
        }
    }

    #[test]
    fn agent_start_sets_running() {
        let mut a = TuiEventAdapter::default();
        a.apply(&AgentEvent::AgentStart(AgentStartEvent {}));
        assert!(a.state().running);
        a.apply(&AgentEvent::AgentEnd(AgentEndEvent { messages: vec![] }));
        assert!(!a.state().running);
    }

    #[test]
    fn text_delta_accumulates_and_finalizes_to_assistant_item() {
        let mut a = TuiEventAdapter::default();
        a.apply(&AgentEvent::MessageStart(MessageStartEvent {
            message: AgentMessage::Assistant(AssistantMessage::default()),
        }));
        a.apply(&AgentEvent::MessageUpdate(Box::new(
            tau_types::MessageUpdateEvent {
                message: AgentMessage::Assistant(AssistantMessage::default()),
                assistant_message_event: AssistantMessageEvent::TextDelta(TextDeltaEvent {
                    content_index: 0,
                    delta: "hello ".to_string(),
                    partial: Arc::new(AssistantMessage::default()),
                }),
            },
        )));
        a.apply(&AgentEvent::MessageUpdate(Box::new(
            tau_types::MessageUpdateEvent {
                message: AgentMessage::Assistant(AssistantMessage::default()),
                assistant_message_event: AssistantMessageEvent::TextDelta(TextDeltaEvent {
                    content_index: 0,
                    delta: "world".to_string(),
                    partial: Arc::new(AssistantMessage::default()),
                }),
            },
        )));
        // Before finalize the buffer holds the full text.
        assert_eq!(a.state().assistant_buffer, "hello world");
        a.apply(&AgentEvent::MessageEnd(MessageEndEvent {
            message: AgentMessage::Assistant(assistant_with_text("hello world")),
        }));
        let items: Vec<_> = a.state().items().iter().collect();
        assert!(
            items
                .iter()
                .any(|i| i.role == ChatItemRole::Assistant && i.text == "hello world")
        );
    }

    #[test]
    fn thinking_delta_appends_thinking_item() {
        let mut a = TuiEventAdapter::default();
        a.apply(&AgentEvent::MessageUpdate(Box::new(
            tau_types::MessageUpdateEvent {
                message: AgentMessage::Assistant(AssistantMessage::default()),
                assistant_message_event: AssistantMessageEvent::ThinkingDelta(ThinkingDeltaEvent {
                    content_index: 0,
                    delta: "hmm".to_string(),
                    partial: Arc::new(AssistantMessage::default()),
                }),
            },
        )));
        assert!(
            a.state()
                .items()
                .iter()
                .any(|i| i.role == ChatItemRole::Thinking && i.text == "hmm")
        );
    }

    #[test]
    fn tool_execution_start_and_end() {
        let mut a = TuiEventAdapter::default();
        a.apply(&AgentEvent::ToolExecutionStart(ToolExecutionStartEvent {
            tool_call_id: "call_1".to_string(),
            tool_name: "read".to_string(),
            args: Default::default(),
        }));
        assert!(
            a.state()
                .items()
                .iter()
                .any(|i| i.role == ChatItemRole::Tool && i.tool_name.as_deref() == Some("read"))
        );
        a.apply(&AgentEvent::ToolExecutionEnd(ToolExecutionEndEvent {
            tool_call_id: "call_1".to_string(),
            tool_name: "read".to_string(),
            result: tau_types::AgentToolResult::from_text("file contents"),
            is_error: false,
        }));
        let tool = a
            .state()
            .items()
            .iter()
            .find(|i| i.tool_call_id.as_deref() == Some("call_1"))
            .unwrap();
        assert!(
            tool.tool_result_text
                .as_deref()
                .unwrap()
                .contains("file contents")
        );
    }

    #[test]
    fn user_message_end_appends_user_item() {
        let mut a = TuiEventAdapter::default();
        a.apply(&AgentEvent::MessageEnd(MessageEndEvent {
            message: AgentMessage::User(UserMessage::new("do a thing")),
        }));
        assert!(
            a.state()
                .items()
                .iter()
                .any(|i| i.role == ChatItemRole::User && i.text == "do a thing")
        );
    }
}
