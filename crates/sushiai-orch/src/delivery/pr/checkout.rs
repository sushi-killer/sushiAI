use std::path::PathBuf;

/// A temporary detached worktree of a branch, removed when dropped.
pub(crate) struct TempWorktree {
    repo: PathBuf,
    pub(crate) path: PathBuf,
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        crate::git::worktree::remove_worktree(&self.repo, &self.path);
    }
}

impl TempWorktree {
    pub(crate) async fn checkout(
        repo: PathBuf,
        path: PathBuf,
        revision: String,
        branch: &str,
    ) -> Result<Self, String> {
        tokio::task::spawn_blocking(move || {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| crate::git::GitError(e.to_string()))?;
            }
            crate::git::worktree::remove_worktree(&repo, &path);
            crate::git::worktree::add_detached_worktree(&repo, &path, &revision)?;
            Ok::<_, crate::git::GitError>(Self { repo, path })
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("could not check out {branch}: {e}"))
    }
}
