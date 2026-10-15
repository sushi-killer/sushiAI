use crate::task::Task;
use std::path::Path;

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::paths::protected_paths;
pub(in crate::engine) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// [`protected_paths`] with their sha256: the planner's files as stored, every
/// other one as it is at `rev`.
pub(in crate::engine) fn protected_hashes(
    worktree: &Path,
    rev: &str,
    task: &Task,
) -> BTreeMap<String, String> {
    protected_paths(task)
        .into_iter()
        .filter_map(|p| {
            let bytes = match task.planner_tests.files.iter().find(|t| t.path == p) {
                Some(t) => Some(t.content.clone().into_bytes()),
                None => crate::git::history::blob_at(worktree, rev, &p),
            };
            bytes.map(|bytes| (p, sha256_hex(&bytes)))
        })
        .collect()
}
