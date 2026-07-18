//! JSONL serialization and Tau-v1 persisted-session migration —
//! `tau_agent.session.jsonl`.
//!
//! `entry_from_json_line` first decodes raw JSON, runs the v1→v2 migration
//! (which only touches `message` entries), then strictly deserializes into a
//! `SessionEntry`. Migration is confined to this persistence boundary so the
//! runtime models remain strict.

use serde_json::{Map, Value};
use tau_types::SessionEntry;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionJsonlError {
    pub line_number: Option<usize>,
    pub message: String,
}

fn err(line_number: Option<usize>, cause: impl std::fmt::Display) -> SessionJsonlError {
    let loc = line_number
        .map(|n| format!(" on line {}", n))
        .unwrap_or_default();
    SessionJsonlError {
        line_number,
        message: format!("Invalid session entry{}: {}", loc, cause),
    }
}

/// Serialize one session entry as one JSONL line (with trailing newline).
pub fn entry_to_json_line(entry: &SessionEntry) -> String {
    let mut s = serde_json::to_string(entry).expect("session entry serializes to JSON");
    s.push('\n');
    s
}

/// Deserialize one entry, migrating persisted Tau-v1 messages first.
pub fn entry_from_json_line(
    line: &str,
    line_number: Option<usize>,
) -> Result<SessionEntry, SessionJsonlError> {
    let value: Value =
        serde_json::from_str(line).map_err(|e| err(line_number, format!("not valid JSON: {e}")))?;
    let migrated = migrate_session_entry(value);
    serde_json::from_value::<SessionEntry>(migrated).map_err(|e| err(line_number, e))
}

/// Deserialize non-empty JSONL lines in order.
pub fn entries_from_json_lines<'a, I>(lines: I) -> Result<Vec<SessionEntry>, SessionJsonlError>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut out = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(entry_from_json_line(line, Some(i + 1))?);
    }
    Ok(out)
}

/// Return a canonical copy of one decoded entry, migrating v1 message shapes.
pub fn migrate_session_entry(value: Value) -> Value {
    let mut map = match value {
        Value::Object(m) => m,
        other => return other,
    };
    let is_message = map
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|s| s == "message");
    if !is_message {
        return Value::Object(map);
    }
    let message = map.remove("message").unwrap_or(Value::Null);
    let migrated_message = migrate_message(message);
    map.insert("message".into(), migrated_message);
    Value::Object(map)
}

fn migrate_message(value: Value) -> Value {
    let mut map = match value {
        Value::Object(m) => m,
        other => return other,
    };
    let role = map
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if role == "user" && (map.contains_key("custom_type") || map.contains_key("customType")) {
        map.insert("role".into(), Value::String("custom".into()));
        let ct = map
            .remove("custom_type")
            .or_else(|| map.get("customType").cloned())
            .unwrap_or(Value::Null);
        map.insert("customType".into(), ct);
        if !map.contains_key("display") {
            map.insert("display".into(), Value::Bool(true));
        }
        return Value::Object(map);
    }

    if role == "assistant" {
        if let Some(Value::Object(usage)) = map.get_mut("usage") {
            let cost_is_null = usage.get("cost").is_none_or(|c| c.is_null());
            if cost_is_null {
                usage.insert("cost".into(), Value::Object(Map::new()));
            }
        }
        let content = map.get("content").cloned().unwrap_or(Value::Null);
        match content {
            Value::String(s) => {
                let mut blocks: Vec<Value> = Vec::new();
                if !s.is_empty() {
                    let mut b = Map::new();
                    b.insert("type".into(), Value::String("text".into()));
                    b.insert("text".into(), Value::String(s));
                    blocks.push(Value::Object(b));
                }
                let t1 = map.remove("tool_calls");
                let t2 = map.remove("toolCalls");
                let tools = t1.or(t2).unwrap_or(Value::Array(Vec::new()));
                if let Some(arr) = tools.as_array() {
                    blocks.extend(arr.iter().cloned());
                }
                map.insert("content".into(), Value::Array(blocks));
            }
            other => {
                if map.contains_key("tool_calls") || map.contains_key("toolCalls") {
                    let mut blocks = match other {
                        Value::Array(a) => a,
                        _ => Vec::new(),
                    };
                    let t1 = map.remove("tool_calls");
                    let t2 = map.remove("toolCalls");
                    let tools = t1.or(t2).unwrap_or(Value::Array(Vec::new()));
                    if let Some(arr) = tools.as_array() {
                        blocks.extend(arr.iter().cloned());
                    }
                    map.insert("content".into(), Value::Array(blocks));
                }
            }
        }
        return Value::Object(map);
    }

    if role == "tool" {
        map.insert("role".into(), Value::String("toolResult".into()));
        let name = map
            .remove("name")
            .or_else(|| map.get("toolName").cloned())
            .unwrap_or(Value::String("unknown".into()));
        map.insert("toolName".into(), name);
        let call_id = map
            .remove("tool_call_id")
            .or_else(|| map.get("toolCallId").cloned())
            .unwrap_or(Value::String(String::new()));
        map.insert("toolCallId".into(), call_id);
        let ok = map.remove("ok").unwrap_or(Value::Bool(true));
        let is_error = !ok.as_bool().unwrap_or(false);
        map.insert("isError".into(), Value::Bool(is_error));

        let content = map.get("content").cloned().unwrap_or(Value::Null);
        if let Value::String(s) = content {
            let blocks = if s.is_empty() {
                Vec::new()
            } else {
                let mut b = Map::new();
                b.insert("type".into(), Value::String("text".into()));
                b.insert("text".into(), Value::String(s));
                vec![Value::Object(b)]
            };
            map.insert("content".into(), Value::Array(blocks));
        }

        let data = map.remove("data");
        let details = map.get("details").cloned();
        match (data, details) {
            (Some(d), Some(det)) if d.is_object() && det.is_object() => {
                let mut merged = d.as_object().unwrap().clone();
                for (k, v) in det.as_object().unwrap() {
                    merged.insert(k.clone(), v.clone());
                }
                map.insert("details".into(), Value::Object(merged));
            }
            (Some(d), None) => {
                map.insert("details".into(), d);
            }
            _ => {}
        }

        let error = map.remove("error");
        if let Some(err_val) = error {
            let content_empty = map
                .get("content")
                .is_none_or(|c| matches!(c, Value::Array(a) if a.is_empty()));
            if content_empty {
                let mut b = Map::new();
                b.insert("type".into(), Value::String("text".into()));
                b.insert("text".into(), Value::String(value_to_text(&err_val)));
                map.insert("content".into(), Value::Array(vec![Value::Object(b)]));
            }
        }
        return Value::Object(map);
    }

    Value::Object(map)
}

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::{AgentMessage, BranchSummaryEntry, EntryType, MessageEntry, ToolResultMessage};

    #[test]
    fn roundtrips_message_entry() {
        let entry = SessionEntry::Message(Box::new(MessageEntry {
            id: "abc".into(),
            parent_id: None,
            timestamp: 1.0,
            r#type: EntryType::Message,
            message: AgentMessage::User(tau_types::UserMessage::new("hi")),
        }));
        let line = entry_to_json_line(&entry);
        let back = entry_from_json_line(line.trim_end(), None).unwrap();
        assert_eq!(back.id(), "abc");
    }

    #[test]
    fn migrates_v1_tool_message_to_tool_result() {
        let line = r#"{"type":"message","id":"e1","timestamp":1.0,"message":{"role":"tool","name":"read","tool_call_id":"c1","content":"done","ok":true}}"#;
        let entry = entry_from_json_line(line, None).unwrap();
        let m = entry.message().unwrap();
        match m {
            AgentMessage::ToolResult(tr) => {
                assert_eq!(tr.tool_name, "read");
                assert_eq!(tr.tool_call_id, "c1");
                assert!(!tr.is_error);
                assert_eq!(tr.text(), "done");
            }
            other => panic!("expected tool result, got {other:?}"),
        }
    }

    #[test]
    fn migrates_v1_assistant_string_content_to_blocks() {
        let line = r#"{"type":"message","id":"e1","timestamp":1.0,"message":{"role":"assistant","content":"hi"}}"#;
        let entry = entry_from_json_line(line, None).unwrap();
        match entry.message().unwrap() {
            AgentMessage::Assistant(a) => assert_eq!(a.text(), "hi"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn migrates_v1_custom_user_to_custom() {
        let line = r#"{"type":"message","id":"e1","timestamp":1.0,"message":{"role":"user","custom_type":"note","content":"x"}}"#;
        let entry = entry_from_json_line(line, None).unwrap();
        match entry.message().unwrap() {
            AgentMessage::Custom(c) => assert_eq!(c.custom_type, "note"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_entry_type() {
        let line = r#"{"type":"nope"}"#;
        let err = entry_from_json_line(line, None).unwrap_err();
        assert!(err.message.contains("unknown session entry type"));
    }

    #[test]
    fn branch_summary_entry_serializes_without_camelcase() {
        let entry = SessionEntry::BranchSummary(BranchSummaryEntry {
            id: "b1".into(),
            parent_id: None,
            timestamp: 1.0,
            r#type: EntryType::BranchSummary,
            summary: "s".into(),
            branch_root_id: Some("root1".into()),
        });
        let line = entry_to_json_line(&entry);
        assert!(line.contains("\"branch_root_id\""));
        assert!(line.contains("\"branch_summary\""));
        // session entries use snake_case (no camelCase aliases at entry level).
        assert!(!line.contains("\"branchRootId\""));
    }

    #[test]
    fn tool_result_message_roundtrip_uses_camelcase() {
        let tr = ToolResultMessage {
            role: tau_types::MessageRole::ToolResult,
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: vec![tau_types::ToolResultContent::Text(
                tau_types::TextContent::new("ok"),
            )],
            details: Value::Object(Map::new()),
            added_tool_names: None,
            is_error: false,
            timestamp: 0,
        };
        let entry = SessionEntry::Message(Box::new(MessageEntry {
            id: "e1".into(),
            parent_id: None,
            timestamp: 1.0,
            r#type: EntryType::Message,
            message: AgentMessage::ToolResult(tr),
        }));
        let line = entry_to_json_line(&entry);
        assert!(line.contains("\"toolCallId\""));
        assert!(line.contains("\"toolName\""));
        assert!(line.contains("\"isError\""));
        let back = entry_from_json_line(line.trim_end(), None).unwrap();
        match back.message().unwrap() {
            AgentMessage::ToolResult(t) => assert_eq!(t.tool_call_id, "c1"),
            _ => panic!(),
        }
    }
}
