//! Translated from `tests/test_agent_harness.py` — tests for the stateful
//! AgentHarness layer: prompt lifecycle, subscribers, overlap rejection,
//! queue modes, tools passing, queue mutators, and interrupted tool repair.

use std::sync::Arc;

use futures::StreamExt;
use tau_types::{
    AgentEvent, AgentMessage, AssistantMessage, MessageEndEvent, MessageStartEvent, TextContent,
    ToolCall,
};

use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::testing::{FakeProvider, assistant_done, assistant_start};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_of(msg: &AgentMessage) -> String {
    match msg {
        AgentMessage::User(u) => u.text().to_string(),
        AgentMessage::Assistant(a) => a.text().to_string(),
        AgentMessage::ToolResult(t) => t.text().to_string(),
        _ => String::new(),
    }
}

fn role_of(msg: &AgentMessage) -> &str {
    match msg {
        AgentMessage::User(_) => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "toolResult",
        _ => "other",
    }
}

fn event_type(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::AgentStart(_) => "agent_start",
        AgentEvent::AgentEnd(_) => "agent_end",
        AgentEvent::TurnStart(_) => "turn_start",
        AgentEvent::TurnEnd(_) => "turn_end",
        AgentEvent::MessageStart(_) => "message_start",
        AgentEvent::MessageEnd(_) => "message_end",
        AgentEvent::MessageUpdate(_) => "message_update",
        AgentEvent::ToolExecutionStart(_) => "tool_execution_start",
        AgentEvent::ToolExecutionUpdate(_) => "tool_execution_update",
        AgentEvent::ToolExecutionEnd(_) => "tool_execution_end",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_prompt_appends_user_and_assistant_with_pi_lifecycle() {
    let provider = FakeProvider::new(vec![vec![
        assistant_start(None, None),
        assistant_done(AssistantMessage::default()),
    ]]);
    let harness = AgentHarness::new(AgentHarnessConfig::new(
        Arc::new(provider),
        "fake",
        "You are Tau.",
    ));

    let events: Vec<AgentEvent> = harness.prompt("Hi").unwrap().collect::<Vec<_>>().await;

    let types: Vec<&str> = events.iter().map(event_type).collect();
    assert_eq!(
        types,
        vec![
            "agent_start",
            "turn_start",
            "message_start",
            "message_end",
            "message_start",
            "message_end",
            "turn_end",
            "agent_end",
        ]
    );

    let starts: Vec<&str> = events
        .iter()
        .filter_map(|ev| match ev {
            AgentEvent::MessageStart(MessageStartEvent { message }) => Some(role_of(message)),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec!["user", "assistant"]);

    let msgs = harness.messages();
    let texts: Vec<(&str, String)> = msgs.iter().map(|m| (role_of(m), text_of(m))).collect();
    assert_eq!(texts, vec![("user", "Hi".into()), ("assistant", "".into())]);
}

#[tokio::test]
async fn test_harness_rejects_overlap_and_drains_followups() {
    let provider = FakeProvider::new(vec![
        vec![
            assistant_start(None, None),
            assistant_done(AssistantMessage::default()),
        ],
        vec![
            assistant_start(None, None),
            assistant_done(AssistantMessage::default()),
        ],
    ]);
    let harness = AgentHarness::new(AgentHarnessConfig::new(
        Arc::new(provider.clone()),
        "fake",
        "You are Tau.",
    ));

    let mut queued = false;
    let mut stream = std::pin::pin!(harness.prompt("Hi").unwrap());
    while let Some(ev) = stream.as_mut().next().await {
        if let AgentEvent::MessageStart(MessageStartEvent {
            message: AgentMessage::Assistant(_),
        }) = &ev
        {
            if !queued {
                // Try to overlap — should fail
                assert!(harness.prompt("overlap").is_err());
                harness.follow_up("Later");
                queued = true;
            }
        }
    }

    let msgs = harness.messages();
    let texts: Vec<(&str, String)> = msgs.iter().map(|m| (role_of(m), text_of(m))).collect();
    assert_eq!(
        texts,
        vec![
            ("user", "Hi".into()),
            ("assistant", "".into()),
            ("user", "Later".into()),
            ("assistant", "".into()),
        ]
    );
}

#[tokio::test]
async fn test_queue_mutators_return_canonical_snapshots() {
    let provider = FakeProvider::new(vec![]);
    let harness = AgentHarness::new(AgentHarnessConfig::new(
        Arc::new(provider),
        "fake",
        "You are Tau.",
    ));

    harness.steer("First");
    harness.steer("Second");
    harness.follow_up("Later");

    let latest_steering = harness.pop_latest_steering().unwrap();
    assert_eq!(text_of(&latest_steering), "Second");
    let latest_follow_up = harness.pop_latest_follow_up().unwrap();
    assert_eq!(text_of(&latest_follow_up), "Later");

    let queued = harness.queued_messages();
    assert_eq!(queued.steering.len(), 1);
    assert_eq!(text_of(&queued.steering[0]), "First");

    let cleared = harness.clear_queues();
    assert_eq!(cleared.steering.len(), 1);
    assert_eq!(text_of(&cleared.steering[0]), "First");
    assert_eq!(harness.pending_message_count(), 0);
}

#[tokio::test]
async fn test_harness_repairs_interrupted_tool_calls() {
    let call = ToolCall::new("call-1", "read");
    let assistant = AssistantMessage {
        content: vec![
            tau_types::AssistantContent::Text(TextContent::new("Reading")),
            tau_types::AssistantContent::ToolCall(call),
        ],
        ..Default::default()
    };

    let provider = FakeProvider::new(vec![]);
    let harness = AgentHarness::with_messages(
        AgentHarnessConfig::new(Arc::new(provider), "fake", "You are Tau."),
        vec![AgentMessage::Assistant(assistant)],
    );

    let repaired = harness.append_interrupted_tool_results();
    assert_eq!(repaired, 1);

    let msgs = harness.messages();
    let repair = msgs.last().unwrap();
    match repair {
        AgentMessage::ToolResult(tr) => {
            assert!(tr.is_error);
            assert!(tr.text().contains("interrupted"));
        }
        other => panic!("expected ToolResultMessage, got {other:?}"),
    }
}

#[tokio::test]
async fn test_harness_queue_mode_all_drains_messages_together() {
    let provider = FakeProvider::new(vec![
        vec![
            assistant_start(None, None),
            assistant_done(AssistantMessage::default()),
        ],
        vec![
            assistant_start(None, None),
            assistant_done(AssistantMessage::default()),
        ],
    ]);
    let harness = AgentHarness::new(AgentHarnessConfig {
        queue_mode: QueueMode::All,
        ..AgentHarnessConfig::new(Arc::new(provider), "fake", "You are Tau.")
    });

    // Drive first prompt, queue follow-ups during the stream
    let mut stream = std::pin::pin!(harness.prompt("Hi").unwrap());
    let mut queued = false;
    while let Some(ev) = stream.as_mut().next().await {
        if !queued {
            if let AgentEvent::MessageEnd(MessageEndEvent {
                message: AgentMessage::Assistant(_),
                ..
            }) = &ev
            {
                harness.follow_up("Second prompt");
                harness.follow_up("Third prompt");
                queued = true;
            }
        }
    }

    let msgs = harness.messages();
    let user_texts: Vec<String> = msgs
        .iter()
        .filter(|m| matches!(m, AgentMessage::User(_)))
        .map(text_of)
        .collect();
    assert_eq!(
        user_texts,
        vec![
            "Hi".to_string(),
            "Second prompt".to_string(),
            "Third prompt".to_string()
        ]
    );
}
