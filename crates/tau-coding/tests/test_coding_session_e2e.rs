//! Phase 5.1 — CodingSession auto-persistence end-to-end test.
//!
//! Drives a real `AgentHarness` backed by `FakeProvider` through
//! `CodingSession::prompt` and verifies that:
//!   - the user prompt and assistant reply both land in the JSONL file;
//!   - the `parent_id` chain weaves user → assistant correctly;
//!   - the harness is the sole driver of persistence (no manual persist
//!     calls happen pre-run).

use std::sync::Arc;

use futures::StreamExt;
use tau_agent::provider::ModelProvider;
use tau_agent::testing::FakeProvider;
use tau_coding::session::{CodingSession, CodingSessionConfig, JsonlSessionStorage};

fn text_done(message: tau_types::AssistantMessage) -> tau_types::AssistantMessageEvent {
    let reason = if message.tool_calls().next().is_some() {
        tau_types::DoneReason::ToolUse
    } else {
        tau_types::DoneReason::Stop
    };
    tau_types::AssistantMessageEvent::Done(tau_types::AssistantDoneEvent { reason, message })
}

#[tokio::test]
async fn prompt_persists_user_and_assistant_with_parent_chain() {
    let dir = tempfile::TempDir::new().unwrap();
    let storage = JsonlSessionStorage::new(dir.path().join("session.jsonl"));

    let mut assistant = tau_types::AssistantMessage::default();
    assistant.content.push(tau_types::AssistantContent::Text(
        tau_types::TextContent::new("hi there"),
    ));
    assistant.stop_reason = tau_types::StopReason::Stop;

    let events = vec![
        tau_agent::testing::assistant_start(None, None),
        tau_agent::testing::text_delta("hi there"),
        text_done(assistant.clone()),
    ];

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(FakeProvider::with_events(events));
    let cfg = CodingSessionConfig {
        provider,
        model: "fake".into(),
        system: None,
        cwd: dir.path().to_path_buf(),
        max_turns: Some(2),
        context_window: None,
        compaction_reserve: 16384,
    };
    let mut session = CodingSession::new(storage, cfg);
    session.write_session_info().await.unwrap();

    {
        let stream = session.prompt("hello").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
        // Stream dropped at end of this block, releasing the `&mut session`.
    }

    let entries = session.storage().read_all().await.unwrap();
    // session_info + user message + leaf + assistant message + leaf
    assert_eq!(entries.len(), 5);

    let user_idx = entries
        .iter()
        .position(|e| matches!(e, tau_types::SessionEntry::Message(m) if matches!(m.message, tau_types::AgentMessage::User(_))))
        .unwrap();
    let assistant_idx = entries
        .iter()
        .position(|e| matches!(e, tau_types::SessionEntry::Message(m) if matches!(m.message, tau_types::AgentMessage::Assistant(_))))
        .unwrap();

    let user_id = entries[user_idx].id().to_string();
    let assistant_entry = match &entries[assistant_idx] {
        tau_types::SessionEntry::Message(m) => m,
        _ => unreachable!(),
    };
    assert_eq!(
        assistant_entry.parent_id.as_deref(),
        Some(user_id.as_str()),
        "assistant message must chain off the user message via parent_id"
    );
}

#[tokio::test]
async fn prompt_persists_multiple_turns_chaining_off_previous_assistant() {
    let dir = tempfile::TempDir::new().unwrap();

    let make_assistant = |text: &str| {
        let mut a = tau_types::AssistantMessage::default();
        a.content.push(tau_types::AssistantContent::Text(
            tau_types::TextContent::new(text),
        ));
        a.stop_reason = tau_types::StopReason::Stop;
        let events = vec![
            tau_agent::testing::assistant_start(None, None),
            tau_agent::testing::text_delta(text),
            text_done(a.clone()),
        ];
        events
    };

    let batch1 = make_assistant("reply one");
    let batch2 = make_assistant("reply two");

    let provider: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(FakeProvider::new(vec![batch1, batch2]));
    let storage = JsonlSessionStorage::new(dir.path().join("multi.jsonl"));
    let cfg = CodingSessionConfig {
        provider,
        model: "fake".into(),
        system: None,
        cwd: dir.path().to_path_buf(),
        max_turns: Some(4),
        context_window: None,
        compaction_reserve: 16384,
    };
    let mut session = CodingSession::new(storage, cfg);
    session.write_session_info().await.unwrap();

    // Turn 1
    {
        let stream = session.prompt("turn 1").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }
    // Turn 2
    {
        let stream = session.prompt("turn 2").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }

    let entries = session.storage().read_all().await.unwrap();
    let ids: Vec<String> = entries.iter().map(|e| e.id().to_string()).collect();
    assert!(ids[0].len() == 32, "expected uuid length");
    // events: session_info, user, leaf, assistant, leaf, user, leaf, assistant, leaf
    assert_eq!(entries.len(), 9);

    // Walk MessageEntries and confirm parent_id points to the previous message.
    let msgs: Vec<(&str, Option<&str>)> = entries
        .iter()
        .filter_map(|e| match e {
            tau_types::SessionEntry::Message(m) => {
                let role = match &m.message {
                    tau_types::AgentMessage::User(_) => "user",
                    tau_types::AgentMessage::Assistant(_) => "assistant",
                    _ => "?",
                };
                Some((role, m.parent_id.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        msgs,
        vec![
            ("user", None),
            ("assistant", Some(ids[1].as_str())),
            ("user", Some(ids[3].as_str())),
            ("assistant", Some(ids[5].as_str())),
        ]
    );
}
