//! Session tree traversal helpers — `tau_agent.session.tree`.

use std::collections::HashMap;

use tau_types::SessionEntry;

#[derive(Debug, thiserror::Error)]
pub enum SessionTreeError {
    #[error("Duplicate session entry id: {0}")]
    DuplicateId(String),
    #[error("Cycle detected at session entry: {0}")]
    CycleDetected(String),
    #[error("Missing session entry: {0}")]
    MissingEntry(String),
}

/// Return entries keyed by id, rejecting duplicates.
pub fn entries_by_id(
    entries: &[SessionEntry],
) -> Result<HashMap<&str, &SessionEntry>, SessionTreeError> {
    let mut result: HashMap<&str, &SessionEntry> = HashMap::new();
    for entry in entries {
        let id = entry.id();
        if result.contains_key(id) {
            return Err(SessionTreeError::DuplicateId(id.to_string()));
        }
        result.insert(id, entry);
    }
    Ok(result)
}

/// Return the root-to-leaf path for `leaf_id`.
pub fn path_to_entry<'a>(
    entries: &'a [SessionEntry],
    leaf_id: &str,
) -> Result<Vec<&'a SessionEntry>, SessionTreeError> {
    let by_id = entries_by_id(entries)?;
    let mut path: Vec<&SessionEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut current_id: Option<&str> = Some(leaf_id);
    while let Some(cid) = current_id {
        if seen.contains(cid) {
            return Err(SessionTreeError::CycleDetected(cid.to_string()));
        }
        seen.insert(cid.to_string());
        let entry = by_id
            .get(cid)
            .ok_or_else(|| SessionTreeError::MissingEntry(cid.to_string()))?;
        path.push(entry);
        current_id = entry.parent_id();
    }
    path.reverse();
    Ok(path)
}
