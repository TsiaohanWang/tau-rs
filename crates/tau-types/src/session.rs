//! Append-only session journal entries (the Pi persisted `SessionEntry` union).
//!
//! Shape: a `type`-tagged discriminated union. Unlike the message/content
//! unions, the entry layer uses **snake_case** field names (there is no Pi
//! `WireModel` alias generator here) — only the nested `message` field is
//! camelCase (it is an `AgentMessage`). Field declaration order matches
//! Python's base-then-subclass ordering for byte-identical serialization.

use std::sync::Arc;

use serde::de;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::message::AgentMessage;

/// Unix epoch seconds (float), matching Python's `time()`.
pub fn current_timestamp_secs() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The `type` discriminator of a `SessionEntry`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Message,
    ModelChange,
    ThinkingLevelChange,
    Compaction,
    BranchSummary,
    Label,
    Leaf,
    SessionInfo,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_message")]
    pub r#type: EntryType,
    pub message: AgentMessage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChangeEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_model_change")]
    pub r#type: EntryType,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThinkingLevelChangeEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_thinking_level_change")]
    pub r#type: EntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactionEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_compaction")]
    pub r#type: EntryType,
    pub summary: String,
    #[serde(default)]
    pub replaces_entry_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BranchSummaryEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_branch_summary")]
    pub r#type: EntryType,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_root_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_label")]
    pub r#type: EntryType,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeafEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_leaf")]
    pub r#type: EntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfoEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_session_info")]
    pub r#type: EntryType,
    #[serde(default = "current_timestamp_secs")]
    pub created_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomEntry {
    #[serde(default = "crate::message::new_entry_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default = "current_timestamp_secs")]
    pub timestamp: f64,
    #[serde(rename = "type", default = "entry_type_custom")]
    pub r#type: EntryType,
    pub namespace: String,
    #[serde(default)]
    pub data: Map<String, Value>,
}

/// The full session journal entry union, discriminated by `type`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SessionEntry {
    Message(Box<MessageEntry>),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Label(LabelEntry),
    Leaf(LeafEntry),
    SessionInfo(SessionInfoEntry),
    Custom(CustomEntry),
}

impl SessionEntry {
    pub fn id(&self) -> &str {
        match self {
            SessionEntry::Message(e) => &e.id,
            SessionEntry::ModelChange(e) => &e.id,
            SessionEntry::ThinkingLevelChange(e) => &e.id,
            SessionEntry::Compaction(e) => &e.id,
            SessionEntry::BranchSummary(e) => &e.id,
            SessionEntry::Label(e) => &e.id,
            SessionEntry::Leaf(e) => &e.id,
            SessionEntry::SessionInfo(e) => &e.id,
            SessionEntry::Custom(e) => &e.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            SessionEntry::Message(e) => e.parent_id.as_deref(),
            SessionEntry::ModelChange(e) => e.parent_id.as_deref(),
            SessionEntry::ThinkingLevelChange(e) => e.parent_id.as_deref(),
            SessionEntry::Compaction(e) => e.parent_id.as_deref(),
            SessionEntry::BranchSummary(e) => e.parent_id.as_deref(),
            SessionEntry::Label(e) => e.parent_id.as_deref(),
            SessionEntry::Leaf(e) => e.parent_id.as_deref(),
            SessionEntry::SessionInfo(e) => e.parent_id.as_deref(),
            SessionEntry::Custom(e) => e.parent_id.as_deref(),
        }
    }

    pub fn entry_type(&self) -> EntryType {
        match self {
            SessionEntry::Message(_) => EntryType::Message,
            SessionEntry::ModelChange(_) => EntryType::ModelChange,
            SessionEntry::ThinkingLevelChange(_) => EntryType::ThinkingLevelChange,
            SessionEntry::Compaction(_) => EntryType::Compaction,
            SessionEntry::BranchSummary(_) => EntryType::BranchSummary,
            SessionEntry::Label(_) => EntryType::Label,
            SessionEntry::Leaf(_) => EntryType::Leaf,
            SessionEntry::SessionInfo(_) => EntryType::SessionInfo,
            SessionEntry::Custom(_) => EntryType::Custom,
        }
    }

    pub fn message(&self) -> Option<&AgentMessage> {
        match self {
            SessionEntry::Message(e) => Some(&e.message),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Manual Deserialize (strict; see ADR-1)
// ---------------------------------------------------------------------------

impl<'de> Deserialize<'de> for SessionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| de::Error::custom("session entry must be a JSON object"))?;
        let tag = obj
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| de::Error::custom("session entry missing `type` field"))?;

        match tag {
            "message" => serde_json::from_value::<MessageEntry>(value)
                .map(|e| SessionEntry::Message(Box::new(e)))
                .map_err(de::Error::custom),
            "model_change" => serde_json::from_value::<ModelChangeEntry>(value)
                .map(SessionEntry::ModelChange)
                .map_err(de::Error::custom),
            "thinking_level_change" => serde_json::from_value::<ThinkingLevelChangeEntry>(value)
                .map(SessionEntry::ThinkingLevelChange)
                .map_err(de::Error::custom),
            "compaction" => serde_json::from_value::<CompactionEntry>(value)
                .map(SessionEntry::Compaction)
                .map_err(de::Error::custom),
            "branch_summary" => serde_json::from_value::<BranchSummaryEntry>(value)
                .map(SessionEntry::BranchSummary)
                .map_err(de::Error::custom),
            "label" => serde_json::from_value::<LabelEntry>(value)
                .map(SessionEntry::Label)
                .map_err(de::Error::custom),
            "leaf" => serde_json::from_value::<LeafEntry>(value)
                .map(SessionEntry::Leaf)
                .map_err(de::Error::custom),
            "session_info" => serde_json::from_value::<SessionInfoEntry>(value)
                .map(SessionEntry::SessionInfo)
                .map_err(de::Error::custom),
            "custom" => serde_json::from_value::<CustomEntry>(value)
                .map(SessionEntry::Custom)
                .map_err(de::Error::custom),
            other => Err(de::Error::custom(format!(
                "unknown session entry type: {other}"
            ))),
        }
    }
}

// per-variant default tag factories
fn entry_type_message() -> EntryType {
    EntryType::Message
}
fn entry_type_model_change() -> EntryType {
    EntryType::ModelChange
}
fn entry_type_thinking_level_change() -> EntryType {
    EntryType::ThinkingLevelChange
}
fn entry_type_compaction() -> EntryType {
    EntryType::Compaction
}
fn entry_type_branch_summary() -> EntryType {
    EntryType::BranchSummary
}
fn entry_type_label() -> EntryType {
    EntryType::Label
}
fn entry_type_leaf() -> EntryType {
    EntryType::Leaf
}
fn entry_type_session_info() -> EntryType {
    EntryType::SessionInfo
}
fn entry_type_custom() -> EntryType {
    EntryType::Custom
}

/// Marker re-export so callers can build `Arc<SessionEntry>` snapshots without
/// importing `std::sync` ad hoc.
pub type SharedEntry = Arc<SessionEntry>;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn message_entry_roundtrip() {
        let msg = AgentMessage::User(crate::UserMessage::new("test message"));
        let entry = MessageEntry {
            id: "id1".into(),
            parent_id: Some("p1".into()),
            timestamp: 1000.0,
            r#type: EntryType::Message,
            message: msg,
        };
        let session = SessionEntry::Message(Box::new(entry));
        let json = serde_json::to_string(&session).unwrap();
        let back: SessionEntry = serde_json::from_str(&json).unwrap();
        match back {
            SessionEntry::Message(e) => {
                assert_eq!(e.parent_id.as_deref(), Some("p1"));
                let mj = serde_json::to_string(&e.message).unwrap();
                assert!(mj.contains("test message"));
            }
            other => panic!("expected Message, got {:?}", other),
        }
    }

    proptest! {
        #[test]
        fn session_entry_never_panics(input in r#""\{.*\}""#) {
            let _ = serde_json::from_str::<SessionEntry>(&input);
        }

        #[test]
        fn message_entry_roundtrips_user_via_proptest(text in ".*") {
            let msg = AgentMessage::User(crate::UserMessage::new(text));
            let entry = MessageEntry {
                id: "proptest-id".into(),
                parent_id: Some("proptest-parent".into()),
                timestamp: 0.0,
                r#type: EntryType::Message,
                message: msg,
            };
            let session = SessionEntry::Message(Box::new(entry));
            let json = serde_json::to_string(&session).unwrap();
            let back: SessionEntry = serde_json::from_str(&json).unwrap();
            match back {
                SessionEntry::Message(e) => {
                    prop_assert_eq!(e.parent_id.as_deref(), Some("proptest-parent"));
                    prop_assert_eq!(e.id, "proptest-id");
                }
                other => prop_assert!(false, "expected Message, got {:?}", other),
            }
        }
    }
}
