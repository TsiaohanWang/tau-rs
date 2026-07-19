//! JSONL-backed session file storage.
//!
//! Each session is a single `.jsonl` file under
//! `~/.tau/sessions/<project_hash>/<session_id>.jsonl`. Entries are appended
//! one per line; reads stream every non-empty line through the v1 migration
//! in `tau_agent::session::jsonl`.

use std::path::{Path, PathBuf};

use tau_agent::session::jsonl::{SessionJsonlError, entry_from_json_line, entry_to_json_line};
use tau_types::SessionEntry;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSONL error: {0}")]
    Jsonl(#[from] SessionJsonlError),

    #[error("Harness error: {0}")]
    Harness(#[from] tau_agent::harness::HarnessError),
}

/// Read-append storage for one session's JSONL journal.
///
/// Concurrent appends are serialized via an in-process `tokio::sync::Mutex`.
/// Cross-process locking is not yet supported (single-writer semantics).
pub struct JsonlSessionStorage {
    path: PathBuf,
    write_lock: tokio::sync::Mutex<()>,
}

impl JsonlSessionStorage {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read every entry, applying v1 migration per line.
    ///
    /// Empty lines and a missing file are tolerated (empty file => empty
    /// result); malformed lines surface as `SessionError::Jsonl`.
    pub async fn read_all(&self) -> Result<Vec<SessionEntry>, SessionError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let text = tokio::fs::read_to_string(&self.path).await?;
        let mut out = Vec::new();
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry = entry_from_json_line(line, Some(i + 1))?;
            out.push(entry);
        }
        Ok(out)
    }

    /// Append a single entry as one JSONL line. Creates the file if missing.
    /// Serialized via an in-process mutex to prevent interleaved writes.
    pub async fn append(&self, entry: &SessionEntry) -> Result<(), SessionError> {
        let line = entry_to_json_line(entry);
        let _guard = self.write_lock.lock().await;
        append_line(&self.path, &line).await
    }

    /// Append multiple entries in one write (still one line per entry).
    pub async fn append_batch(&self, entries: &[SessionEntry]) -> Result<(), SessionError> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut buf = String::new();
        for e in entries {
            buf.push_str(&entry_to_json_line(e));
        }
        let _guard = self.write_lock.lock().await;
        append_line(&self.path, buf.trim_end_matches('\n')).await?;
        Ok(())
    }
}

async fn append_line(path: &Path, line: &str) -> Result<(), SessionError> {
    use tokio::io::AsyncWriteExt;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    if !line.ends_with('\n') {
        file.write_all(b"\n").await?;
    }
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tau_types::{
        AgentMessage, EntryType, LeafEntry, MessageEntry, SessionInfoEntry, UserMessage,
    };
    use tempfile::TempDir;

    fn info_entry() -> SessionEntry {
        SessionEntry::SessionInfo(SessionInfoEntry {
            id: "s1".into(),
            parent_id: None,
            timestamp: 1.0,
            r#type: EntryType::SessionInfo,
            created_at: 1.0,
            cwd: Some("/tmp".into()),
            title: None,
        })
    }

    fn message_entry(text: &str) -> SessionEntry {
        SessionEntry::Message(Box::new(MessageEntry {
            id: format!("m-{text}"),
            parent_id: None,
            timestamp: 2.0,
            r#type: EntryType::Message,
            message: AgentMessage::User(UserMessage::new(text)),
        }))
    }

    fn leaf_entry() -> SessionEntry {
        SessionEntry::Leaf(LeafEntry {
            id: "l1".into(),
            parent_id: None,
            timestamp: 3.0,
            r#type: EntryType::Leaf,
            entry_id: Some("m-hi".into()),
        })
    }

    #[tokio::test]
    async fn read_all_missing_file_returns_empty() {
        let dir = TempDir::new().unwrap();
        let storage = JsonlSessionStorage::new(dir.path().join("nope.jsonl"));
        assert!(storage.read_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn append_then_read_roundtrips() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("session.jsonl");
        let storage = JsonlSessionStorage::new(path);
        storage.append(&info_entry()).await.unwrap();
        storage.append(&message_entry("hi")).await.unwrap();
        storage.append(&leaf_entry()).await.unwrap();
        let entries = storage.read_all().await.unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id(), "s1");
        assert_eq!(entries[1].id(), "m-hi");
        assert_eq!(entries[2].id(), "l1");
    }

    #[tokio::test]
    async fn append_batch_writes_one_line_per_entry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("batch.jsonl");
        let storage = JsonlSessionStorage::new(path.clone());
        let batch = vec![info_entry(), message_entry("a"), message_entry("b")];
        storage.append_batch(&batch).await.unwrap();
        let text = tokio::fs::read_to_string(&path).await.unwrap();
        assert_eq!(text.lines().count(), 3);
        let entries = storage.read_all().await.unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn empty_lines_are_ignored() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gaps.jsonl");
        tokio::fs::write(
            &path,
            format!(
                "{}\n\n{}\n\n",
                entry_to_json_line(&info_entry()).trim_end(),
                entry_to_json_line(&leaf_entry()).trim_end(),
            ),
        )
        .await
        .unwrap();
        let storage = JsonlSessionStorage::new(path);
        let entries = storage.read_all().await.unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn malformed_line_surfaces_error_with_line_number() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.jsonl");
        let ok = entry_to_json_line(&info_entry()).trim_end().to_string();
        let bad = r#"{"type":"message","id":"x","timestamp":0.0,"message":{"role":"nope"}}"#;
        tokio::fs::write(&path, format!("{ok}\n{bad}\n"))
            .await
            .unwrap();
        let storage = JsonlSessionStorage::new(path);
        let err = storage.read_all().await.unwrap_err();
        match err {
            SessionError::Jsonl(e) => assert_eq!(e.line_number, Some(2)),
            other => panic!("expected Jsonl error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn append_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deep/session.jsonl");
        let storage = JsonlSessionStorage::new(path);
        storage.append(&info_entry()).await.unwrap();
        assert!(storage.path().exists());
    }
}
