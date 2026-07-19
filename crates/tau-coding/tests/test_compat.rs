//! Bidirectional compatibility tests (Phase 5.6).
//!
//! Proves that:
//! 1. A Rust-serialized session JSONL is byte-stable (deterministic) across
//!    runs — assistant messages use a fixed timestamp so the golden bytes do
//!    not drift — and round-trips back to the identical bytes (golden). This
//!    locks the wire format: any change to field ordering / camelCase /
//!    skip-None breaks these tests.
//! 2. v1 Python shapes (string `content` for assistant) also load via the
//!    migration path.
//! 3. A `CodingSession::load` over a Python-style file reconstructs the
//!    conversation so it can be resumed.

use std::sync::Arc;

use tau_agent::session::jsonl::{
    entries_from_json_lines, entry_from_json_line, entry_to_json_line,
};
use tau_agent::testing::FakeProvider;
use tau_coding::session::{CodingSession, CodingSessionConfig};
use tau_coding::tools::create_coding_tools;
use tau_types::{
    AgentMessage, AssistantContent, EntryType, LeafEntry, MessageEntry, MessageRole, SessionEntry,
    SessionInfoEntry, TextContent, ToolCall, ToolResultContent, ToolResultMessage, UserContent,
    UserMessage,
};

/// Fixed timestamp (ms) injected into assistant messages so their serialized
/// form is deterministic for the golden round-trip.
const ASSISTANT_TS: i64 = 1_700_000_000_000;
const TOOL_RESULT_TS: i64 = 1_700_000_000_000;

fn session_info(id: &str, ts: f64, cwd: Option<&str>) -> SessionEntry {
    SessionEntry::SessionInfo(SessionInfoEntry {
        id: id.into(),
        parent_id: None,
        timestamp: ts,
        r#type: EntryType::SessionInfo,
        created_at: ts,
        cwd: cwd.map(String::from),
        title: None,
    })
}

fn user_msg(id: &str, parent: Option<&str>, ts: f64, text: &str) -> SessionEntry {
    SessionEntry::Message(Box::new(MessageEntry {
        id: id.into(),
        parent_id: parent.map(String::from),
        timestamp: ts,
        r#type: EntryType::Message,
        message: AgentMessage::User(UserMessage {
            role: MessageRole::User,
            content: UserContent::Text(text.into()),
            timestamp: (ts * 1000.0) as i64,
        }),
    }))
}

fn assistant_msg(id: &str, parent: Option<&str>, ts: f64, text: &str) -> SessionEntry {
    let mut a = tau_types::AssistantMessage::default();
    a.content
        .push(AssistantContent::Text(TextContent::new(text)));
    a.timestamp = ASSISTANT_TS;
    SessionEntry::Message(Box::new(MessageEntry {
        id: id.into(),
        parent_id: parent.map(String::from),
        timestamp: ts,
        r#type: EntryType::Message,
        message: AgentMessage::Assistant(a),
    }))
}

fn assistant_with_tool(
    id: &str,
    parent: Option<&str>,
    ts: f64,
    text: &str,
    tool_name: &str,
    call_id: &str,
    args: serde_json::Value,
) -> SessionEntry {
    let mut a = tau_types::AssistantMessage::default();
    a.content
        .push(AssistantContent::Text(TextContent::new(text)));
    a.content.push(AssistantContent::ToolCall(ToolCall::new(
        call_id, tool_name,
    )));
    if let AssistantContent::ToolCall(tc) = &mut a.content[1] {
        if let serde_json::Value::Object(map) = args {
            tc.arguments = map;
        }
    }
    a.timestamp = ASSISTANT_TS;
    SessionEntry::Message(Box::new(MessageEntry {
        id: id.into(),
        parent_id: parent.map(String::from),
        timestamp: ts,
        r#type: EntryType::Message,
        message: AgentMessage::Assistant(a),
    }))
}

fn tool_result(
    id: &str,
    parent: Option<&str>,
    ts: f64,
    call_id: &str,
    tool_name: &str,
    text: &str,
    is_error: bool,
) -> SessionEntry {
    let mut tr = ToolResultMessage::new(call_id, tool_name);
    tr.content
        .push(ToolResultContent::Text(TextContent::new(text)));
    tr.is_error = is_error;
    tr.timestamp = TOOL_RESULT_TS;
    SessionEntry::Message(Box::new(MessageEntry {
        id: id.into(),
        parent_id: parent.map(String::from),
        timestamp: ts,
        r#type: EntryType::Message,
        message: AgentMessage::ToolResult(tr),
    }))
}

fn leaf(id: &str, ts: f64, entry_id: &str) -> SessionEntry {
    SessionEntry::Leaf(LeafEntry {
        id: id.into(),
        parent_id: None,
        timestamp: ts,
        r#type: EntryType::Leaf,
        entry_id: Some(entry_id.into()),
    })
}

fn short_entries() -> Vec<SessionEntry> {
    vec![
        session_info("s1", 1000.0, Some("/home/user/proj")),
        user_msg("m1", None, 1001.0, "Hello"),
        assistant_msg("m2", Some("m1"), 1002.0, "Hi there"),
        leaf("l1", 1003.0, "m2"),
    ]
}

fn tool_entries() -> Vec<SessionEntry> {
    vec![
        session_info("s2", 2000.0, Some("/home/user/proj")),
        user_msg("m3", None, 2001.0, "Read the file"),
        assistant_with_tool(
            "m4",
            Some("m3"),
            2002.0,
            "Sure",
            "read",
            "c1",
            serde_json::json!({"path":"a.txt"}),
        ),
        tool_result(
            "m5",
            Some("m4"),
            2003.0,
            "c1",
            "read",
            "file contents",
            false,
        ),
        leaf("l2", 2004.0, "m5"),
    ]
}

/// Captured golden lines: `entry_to_json_line` output for the builders above.
/// Regenerate with `dump_golden_lines` if the wire format is intentionally
/// changed.
const SHORT_GOLDEN: &[&str] = &[
    r#"{"id":"s1","timestamp":1000.0,"type":"session_info","created_at":1000.0,"cwd":"/home/user/proj"}"#,
    r#"{"id":"m1","timestamp":1001.0,"type":"message","message":{"role":"user","content":"Hello","timestamp":1001000}}"#,
    r#"{"id":"m2","parent_id":"m1","timestamp":1002.0,"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Hi there"}],"api":"unknown","provider":"unknown","model":"unknown","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"stopReason":"stop","timestamp":1700000000000}}"#,
    r#"{"id":"l1","timestamp":1003.0,"type":"leaf","entry_id":"m2"}"#,
];

const TOOL_GOLDEN: &[&str] = &[
    r#"{"id":"s2","timestamp":2000.0,"type":"session_info","created_at":2000.0,"cwd":"/home/user/proj"}"#,
    r#"{"id":"m3","timestamp":2001.0,"type":"message","message":{"role":"user","content":"Read the file","timestamp":2001000}}"#,
    r#"{"id":"m4","parent_id":"m3","timestamp":2002.0,"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Sure"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"a.txt"}}],"api":"unknown","provider":"unknown","model":"unknown","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"stopReason":"stop","timestamp":1700000000000}}"#,
    r#"{"id":"m5","parent_id":"m4","timestamp":2003.0,"type":"message","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"file contents"}],"isError":false,"timestamp":1700000000000}}"#,
    r#"{"id":"l2","timestamp":2004.0,"type":"leaf","entry_id":"m5"}"#,
];

#[test]
fn golden_short_conversation_roundtrips_byte_for_byte() {
    let entries = short_entries();
    assert_eq!(entries.len(), SHORT_GOLDEN.len());
    for (entry, expected) in entries.iter().zip(SHORT_GOLDEN) {
        let out = entry_to_json_line(entry).trim_end().to_string();
        assert_eq!(out, *expected, "short golden line mismatch");
        // And it parses back to the same bytes.
        let reparsed = entry_from_json_line(expected, None).expect("golden line parses");
        assert_eq!(
            entry_to_json_line(&reparsed).trim_end(),
            *expected,
            "re-serialized bytes must equal the golden fixture"
        );
    }
}

#[test]
fn golden_tool_conversation_roundtrips_byte_for_byte() {
    let entries = tool_entries();
    assert_eq!(entries.len(), TOOL_GOLDEN.len());
    for (entry, expected) in entries.iter().zip(TOOL_GOLDEN) {
        let out = entry_to_json_line(entry).trim_end().to_string();
        assert_eq!(out, *expected, "tool golden line mismatch");
        let reparsed = entry_from_json_line(expected, None).expect("golden line parses");
        assert_eq!(
            entry_to_json_line(&reparsed).trim_end(),
            *expected,
            "re-serialized bytes must equal the golden fixture"
        );
    }
}

#[test]
fn reads_python_style_short_conversation() {
    let entries = entries_from_json_lines(SHORT_GOLDEN.iter().copied()).expect("all lines parse");
    assert_eq!(entries.len(), 4);
    let msgs: Vec<&AgentMessage> = entries
        .iter()
        .filter_map(|e| match e {
            SessionEntry::Message(m) => Some(&m.message),
            _ => None,
        })
        .collect();
    assert_eq!(msgs.len(), 2);
    match (&msgs[0], &msgs[1]) {
        (AgentMessage::User(u), AgentMessage::Assistant(a)) => {
            assert_eq!(u.text(), "Hello");
            assert_eq!(a.text(), "Hi there");
        }
        other => panic!("unexpected roles: {other:?}"),
    }
}

#[test]
fn reads_python_style_tool_conversation() {
    let entries = entries_from_json_lines(TOOL_GOLDEN.iter().copied()).expect("all lines parse");
    let msgs: Vec<&AgentMessage> = entries
        .iter()
        .filter_map(|e| match e {
            SessionEntry::Message(m) => Some(&m.message),
            _ => None,
        })
        .collect();
    assert_eq!(msgs.len(), 3);
    assert!(matches!(msgs[0], AgentMessage::User(_)));
    assert!(matches!(msgs[1], AgentMessage::Assistant(_)));
    match &msgs[1] {
        AgentMessage::Assistant(a) => {
            assert!(
                a.content
                    .iter()
                    .any(|c| matches!(c, AssistantContent::ToolCall(_)))
            );
        }
        other => panic!("expected assistant with tool call, got {other:?}"),
    }
    match &msgs[2] {
        AgentMessage::ToolResult(tr) => {
            assert_eq!(tr.tool_name, "read");
            assert_eq!(tr.tool_call_id, "c1");
            assert!(!tr.is_error);
            assert_eq!(tr.text(), "file contents");
        }
        other => panic!("expected tool result, got {other:?}"),
    }
}

#[test]
fn reads_v1_assistant_string_content() {
    // Python v1 often writes assistant `content` as a bare string.
    let line = r#"{"type":"message","id":"e1","timestamp":1.0,"message":{"role":"assistant","content":"hi"}}"#;
    let entry = entry_from_json_line(line, None).expect("v1 assistant parses");
    match entry.message().unwrap() {
        AgentMessage::Assistant(a) => assert_eq!(a.text(), "hi"),
        other => panic!("expected assistant, got {other:?}"),
    }
}

#[tokio::test]
async fn resume_loads_python_style_session() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("python_session.jsonl");
    let mut text = String::new();
    for l in SHORT_GOLDEN {
        text.push_str(l);
        text.push('\n');
    }
    std::fs::write(&path, text).unwrap();

    let storage = tau_coding::session::JsonlSessionStorage::new(path);
    let provider: Arc<dyn tau_agent::provider::ModelProvider + Send + Sync> =
        Arc::new(FakeProvider::new(vec![]));
    let cwd = std::path::PathBuf::from("/home/user/proj");
    let config = CodingSessionConfig {
        provider,
        model: "fake".into(),
        system: None,
        cwd: cwd.clone(),
        max_turns: Some(4),
        context_window: None,
        compaction_reserve: 16384,
        provider_name: None,
        thinking_level: None,
    };
    let session = CodingSession::load(storage, config)
        .await
        .expect("python-style session loads");

    // The two reconstructed messages are replayable.
    let messages = session.messages();
    assert_eq!(messages.len(), 2, "loaded session should have 2 messages");
    assert!(matches!(messages[0], AgentMessage::User(_)));
    assert!(matches!(messages[1], AgentMessage::Assistant(_)));

    // tools are wired so the session can continue.
    let _ = create_coding_tools(&cwd);
}
