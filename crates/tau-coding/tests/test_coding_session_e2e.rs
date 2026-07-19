//! Phase 5 — CodingSession end-to-end + load/resume tests.

use std::sync::Arc;

use futures::StreamExt;
use tau_agent::provider::ModelProvider;
use tau_agent::testing::FakeProvider;
use tau_coding::session::repair_interrupted_tool_calls;
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
        provider_name: None,
    };
    let mut session = CodingSession::new(storage, cfg);
    session.write_session_info().await.unwrap();

    {
        let stream = session.prompt("hello").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }

    let entries = session.storage().read_all().await.unwrap();
    // session_info(1) + auto-title Label(1) + (user, leaf, assistant, leaf)(4) = 6
    assert_eq!(entries.len(), 6);

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
        vec![
            tau_agent::testing::assistant_start(None, None),
            tau_agent::testing::text_delta(text),
            text_done(a.clone()),
        ]
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
        provider_name: None,
    };
    let mut session = CodingSession::new(storage, cfg);
    session.write_session_info().await.unwrap();

    {
        let stream = session.prompt("turn 1").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }
    {
        let stream = session.prompt("turn 2").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }

    let entries = session.storage().read_all().await.unwrap();
    let ids: Vec<String> = entries.iter().map(|e| e.id().to_string()).collect();
    assert_eq!(ids[0].len(), 32, "expected uuid length");
    // session_info(1) + auto-title Label(1) + (user,leaf,assistant,leaf) x 2 = 10
    assert_eq!(entries.len(), 10);

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
            // first user message chains off the auto-title Label entry
            ("user", Some(ids[1].as_str())),
            ("assistant", Some(ids[2].as_str())),
            ("user", Some(ids[4].as_str())),
            ("assistant", Some(ids[6].as_str())),
        ]
    );
}

// ---------------------------------------------------------------------------
// Phase 5.2 — session load / resume
// ---------------------------------------------------------------------------

#[tokio::test]
async fn load_reconstructs_prior_conversation_in_harness_messages() {
    let dir = tempfile::TempDir::new().unwrap();
    let storage = JsonlSessionStorage::new(dir.path().join("resume.jsonl"));

    let mut first_assistant = tau_types::AssistantMessage::default();
    first_assistant
        .content
        .push(tau_types::AssistantContent::Text(
            tau_types::TextContent::new("ok"),
        ));
    first_assistant.stop_reason = tau_types::StopReason::Stop;

    let first_events = vec![
        tau_agent::testing::assistant_start(None, None),
        tau_agent::testing::text_delta("ok"),
        text_done(first_assistant.clone()),
    ];

    let provider_first: Arc<dyn ModelProvider + Send + Sync> =
        Arc::new(FakeProvider::with_events(first_events));
    let mut session = CodingSession::new(
        JsonlSessionStorage::new(storage.path().to_path_buf()),
        CodingSessionConfig {
            provider: provider_first,
            model: "fake".into(),
            system: None,
            cwd: dir.path().to_path_buf(),
            max_turns: Some(2),
            context_window: None,
            compaction_reserve: 16384,
            provider_name: None,
        },
    );
    session.write_session_info().await.unwrap();
    {
        let stream = session.prompt("first message").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }
    drop(session);

    // Now `load` the same file.
    let loaded = CodingSession::load(
        JsonlSessionStorage::new(storage.path().to_path_buf()),
        CodingSessionConfig {
            provider: Arc::new(FakeProvider::with_events(vec![text_done(
                first_assistant.clone(),
            )])) as _,
            model: "fake".into(),
            system: None,
            cwd: dir.path().to_path_buf(),
            max_turns: Some(2),
            context_window: None,
            compaction_reserve: 16384,
            provider_name: None,
        },
    )
    .await
    .unwrap();

    // Loaded harness messages should contain the original user + assistant.
    let msgs = loaded.messages();
    assert!(
        msgs.iter()
            .any(|m| matches!(m, tau_types::AgentMessage::User(_))),
        "loaded session must see the prior user message"
    );
    assert!(
        msgs.iter()
            .any(|m| matches!(m, tau_types::AgentMessage::Assistant(_))),
        "loaded session must see the prior assistant message"
    );

    // A new prompt on the resumed session chains off the persisted tail:
    // first event persisted (the user MessageEntry) should have a real
    // parent_id (not None), because `last_entry_id` was hydrated by load.
    let mut resumed = loaded;
    {
        let mut a = tau_types::AssistantMessage::default();
        a.content.push(tau_types::AssistantContent::Text(
            tau_types::TextContent::new("two"),
        ));
        a.stop_reason = tau_types::StopReason::Stop;
        let events = vec![text_done(a)];
        // Replace provider in the running harness — easiest path is to drop
        // and re-load with a brand-new FakeProvider that serves the next reply.
        let provider_next: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(FakeProvider::with_events(events));
        let cfg = CodingSessionConfig {
            provider: provider_next,
            model: "fake".into(),
            system: None,
            cwd: dir.path().to_path_buf(),
            max_turns: Some(2),
            context_window: None,
            compaction_reserve: 16384,
            provider_name: None,
        };
        // Swap provider & rebuild: easiest re-load (load keeps the file as-is).
        // This relies on `load` reconstructing from the file — the harness
        // provider is whatever we pass to `load`, so we re-load once more.
        drop(resumed);
        resumed = CodingSession::load(JsonlSessionStorage::new(storage.path().to_path_buf()), cfg)
            .await
            .unwrap();
    }
    {
        let stream = resumed.prompt("second message").unwrap();
        futures::pin_mut!(stream);
        while let Some(_ev) = stream.next().await {}
    }

    let entries = resumed.storage().read_all().await.unwrap();
    let last_user_msg = entries
        .iter()
        .rev()
        .find(|e| matches!(e, tau_types::SessionEntry::Message(m) if matches!(m.message, tau_types::AgentMessage::User(_))));
    let last_user = match last_user_msg {
        Some(tau_types::SessionEntry::Message(m)) => m,
        _ => panic!("expected a MessageEntry for the second-turn user"),
    };
    assert!(
        last_user.parent_id.is_some(),
        "second-turn user message must chain off the persisted tail (last_entry_id from load)"
    );
}

#[test]
fn repair_interrupted_tool_call_inserts_synthetic_error_result() {
    let mut a = tau_types::AssistantMessage::default();
    a.content
        .push(tau_types::AssistantContent::ToolCall(tau_types::ToolCall {
            id: "c1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
            r#type: tau_types::ContentBlockType::ToolCall,
        }));
    a.stop_reason = tau_types::StopReason::ToolUse;
    let mut msgs = vec![
        tau_types::AgentMessage::User(tau_types::UserMessage::new("go")),
        tau_types::AgentMessage::Assistant(a),
    ];
    let repaired = repair_interrupted_tool_calls(&mut msgs);
    assert_eq!(repaired, vec!["c1".to_string()]);
    assert_eq!(msgs.len(), 3);
    assert!(matches!(
        &msgs[2],
        tau_types::AgentMessage::ToolResult(t) if t.is_error && t.tool_call_id == "c1"
    ));
}
