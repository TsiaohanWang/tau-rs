//! Session directory management.
//!
//! Layout mirrors the Python `SessionManager`:
//!
//! ```text
//! ~/.tau/sessions/
//!   <project_hash>/
//!     index.jsonl
//!     <session_id>.jsonl
//! ```
//!
//! The `project_hash` is the first 12 hex chars of the SHA-256 of the
//! project directory's canonical absolute path. `index.jsonl` is a JSON-line
//! append-only list of [`SessionIndexEntry`] rows, one per session file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session::storage::{JsonlSessionStorage, SessionError};

/// Index-row appended to `index.jsonl` for each new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub session_path: String,
    pub created_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Summary of a listed session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: f64,
    pub title: Option<String>,
    pub entry_count: usize,
}

/// Owns the sessions root and creates/loads per-project session journals.
pub struct SessionManager {
    sessions_dir: PathBuf,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// Stable per-project subdirectory name (first 12 hex chars of SHA-256).
    pub fn project_hash(project_dir: &Path) -> String {
        use sha2::{Digest, Sha256};
        let canonical = project_dir.to_string_lossy().into_owned();
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        let digest = hasher.finalize();
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        hex.chars().take(12).collect()
    }

    /// Return the per-project directory, creating it if missing.
    pub async fn prepare(&self, project_dir: &Path) -> Result<PathBuf, SessionError> {
        let hash = Self::project_hash(project_dir);
        let dir = self.sessions_dir.join(&hash);
        tokio::fs::create_dir_all(&dir).await?;
        Ok(dir)
    }

    /// Create a brand-new session: pick a uuid, build the storage, append an
    /// index row. Returns `(session_path, storage)`.
    pub async fn create(
        &self,
        project_dir: &Path,
    ) -> Result<(PathBuf, JsonlSessionStorage), SessionError> {
        let dir = self.prepare(project_dir).await?;
        let session_id = uuid::Uuid::new_v4().simple().to_string();
        let session_path = dir.join(format!("{session_id}.jsonl"));
        let storage = JsonlSessionStorage::new(session_path.clone());

        let index_entry = SessionIndexEntry {
            session_id: session_id.clone(),
            session_path: format!("{session_id}.jsonl"),
            created_at: tau_types::current_timestamp_secs(),
            cwd: Some(project_dir.to_str().unwrap_or("").to_string()),
            title: None,
        };
        self.append_to_index(project_dir, &index_entry).await?;
        Ok((session_path, storage))
    }

    /// Open storage for an already-existing session file.
    pub async fn load(
        &self,
        project_dir: &Path,
        session_id: &str,
    ) -> Result<JsonlSessionStorage, SessionError> {
        let dir = self.prepare(project_dir).await?;
        let path = dir.join(format!("{session_id}.jsonl"));
        Ok(JsonlSessionStorage::new(path))
    }

    /// Read every row of `index.jsonl` for a project.
    pub async fn load_index(
        &self,
        project_dir: &Path,
    ) -> Result<Vec<SessionIndexEntry>, SessionError> {
        let dir = self.prepare(project_dir).await?;
        let index_path = dir.join("index.jsonl");
        if !tokio::fs::try_exists(&index_path).await? {
            return Ok(Vec::new());
        }
        let text = tokio::fs::read_to_string(index_path).await?;
        let mut out = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: SessionIndexEntry = serde_json::from_str(line).map_err(|e| {
                SessionError::Jsonl(tau_agent::session::jsonl::SessionJsonlError {
                    line_number: None,
                    message: format!("index.jsonl: {e}"),
                })
            })?;
            out.push(entry);
        }
        Ok(out)
    }

    /// Append one row to `index.jsonl`.
    pub async fn append_to_index(
        &self,
        project_dir: &Path,
        entry: &SessionIndexEntry,
    ) -> Result<(), SessionError> {
        let dir = self.prepare(project_dir).await?;
        let index_path = dir.join("index.jsonl");
        let mut line = serde_json::to_string(entry).expect("index entry serializes");
        line.push('\n');
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(index_path)
            .await?;
        file.write_all(line.as_bytes()).await?;
        Ok(())
    }

    /// List sessions for a project, counted entries each.
    pub async fn list(&self, project_dir: &Path) -> Result<Vec<SessionInfo>, SessionError> {
        let dir = self.prepare(project_dir).await?;
        let index = self.load_index(project_dir).await?;
        let mut out = Vec::with_capacity(index.len());
        for row in index {
            let path = dir.join(&row.session_path);
            let count = if tokio::fs::try_exists(&path).await? {
                tokio::fs::read_to_string(&path)
                    .await?
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .count()
            } else {
                0
            };
            out.push(SessionInfo {
                session_id: row.session_id,
                created_at: row.created_at,
                title: row.title,
                entry_count: count,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp_root() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn project_hash_is_stable_and_12_chars() {
        let h1 = SessionManager::project_hash(Path::new("/foo/bar"));
        let h2 = SessionManager::project_hash(Path::new("/foo/bar"));
        let h3 = SessionManager::project_hash(Path::new("/foo/baz"));
        assert_eq!(h1.len(), 12);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[tokio::test]
    async fn create_writes_index_row_and_session_file() {
        let root = tmp_root();
        let mgr = SessionManager::new(root.path().to_path_buf());
        let project = Path::new("/tmp/project");

        let (path, storage) = mgr.create(project).await.unwrap();
        // The session file is not created until something is appended.
        assert!(
            !path.exists(),
            "session file is lazily created on first append"
        );
        assert_eq!(storage.path(), &path);

        let index = mgr.load_index(project).await.unwrap();
        assert_eq!(index.len(), 1);
        assert_eq!(
            index[0].session_id,
            path.file_stem().unwrap().to_string_lossy()
        );
    }

    #[tokio::test]
    async fn load_returns_storage_pointing_at_existing_file() {
        let root = tmp_root();
        let mgr = SessionManager::new(root.path().to_path_buf());
        let project = Path::new("/tmp/proj2");

        let (path, storage) = mgr.create(project).await.unwrap();
        storage
            .append(&tau_types::SessionEntry::Leaf(tau_types::LeafEntry {
                id: "l1".into(),
                parent_id: None,
                timestamp: 1.0,
                r#type: tau_types::EntryType::Leaf,
                entry_id: None,
            }))
            .await
            .unwrap();

        let reloaded = mgr
            .load(project, path.file_stem().unwrap().to_str().unwrap())
            .await
            .unwrap();
        let entries = reloaded.read_all().await.unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[tokio::test]
    async fn list_counts_entries_per_session() {
        let root = tmp_root();
        let mgr = SessionManager::new(root.path().to_path_buf());
        let project = Path::new("/tmp/proj3");

        let (_p1, s1) = mgr.create(project).await.unwrap();
        let (_p2, s2) = mgr.create(project).await.unwrap();
        // s1 -> empty, s2 -> append two lines via storage
        {
            let _ = s1;
            let e1 = tau_types::SessionEntry::Leaf(tau_types::LeafEntry {
                id: "a".into(),
                parent_id: None,
                timestamp: 1.0,
                r#type: tau_types::EntryType::Leaf,
                entry_id: None,
            });
            let e2 = tau_types::SessionEntry::Leaf(tau_types::LeafEntry {
                id: "b".into(),
                parent_id: None,
                timestamp: 2.0,
                r#type: tau_types::EntryType::Leaf,
                entry_id: None,
            });
            s2.append(&e1).await.unwrap();
            s2.append(&e2).await.unwrap();
        }

        let infos = mgr.list(project).await.unwrap();
        assert_eq!(infos.len(), 2);
        let total: usize = infos.iter().map(|i| i.entry_count).sum();
        assert_eq!(total, 2);
    }

    #[tokio::test]
    async fn load_index_missing_returns_empty() {
        let root = tmp_root();
        let mgr = SessionManager::new(root.path().to_path_buf());
        let project = Path::new("/tmp/no-such");
        assert!(mgr.load_index(project).await.unwrap().is_empty());
    }
}
