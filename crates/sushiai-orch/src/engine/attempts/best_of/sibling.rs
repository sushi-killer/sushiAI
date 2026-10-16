use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::PlannedTest;
use crate::task::Task;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// The second candidate's worktree: the task's base with the planner's test
/// files written over it, as the task's own worktree has them.
pub(super) fn create_sibling(
    repo: &Path,
    branch: &str,
    path: &Path,
    base_sha: &str,
    files: &[PlannedTest],
) -> Result<(), crate::git::GitError> {
    crate::git::worktree::create_worktree(repo, branch, path, base_sha)?;
    if let Err(e) = crate::git::bootstrap::bootstrap_worktree(repo, path)
        .map_err(|e| e.to_string())
        .and_then(|()| crate::engine::protected::prepare::write_files(path, files).map(|_| ()))
    {
        crate::delivery::git::recovery::discard_worktree(repo, path, branch);
        return Err(crate::git::GitError(e));
    }
    Ok(())
}

pub(super) struct Sibling {
    pub(super) worktree: PathBuf,
    pub(super) run_dir: PathBuf,
    repo: PathBuf,
    branch: String,
}
impl Sibling {
    pub(super) async fn create(
        app: &Arc<App>,
        task: &Task,
        run_dir: &Path,
        cancel: &CancelToken,
    ) -> Result<Self, crate::git::GitError> {
        let sibling = Self {
            repo: PathBuf::from(&task.repo),
            worktree: PathBuf::from(format!("{}-b", task.worktree)),
            branch: format!("{}-b", task.branch),
            run_dir: run_dir.join("b"),
        };
        let (repo, path, branch, base, files) = (
            sibling.repo.clone(),
            sibling.worktree.clone(),
            sibling.branch.clone(),
            task.base_sha.clone(),
            task.planner_tests.files.clone(),
        );
        tokio::task::spawn_blocking(move || create_sibling(&repo, &branch, &path, &base, &files))
            .await
            .unwrap_or_else(|e| Err(crate::git::GitError(e.to_string())))?;
        let setup = crate::engine::worktrees::setup_worktree(
            &sibling.repo,
            &sibling.worktree,
            app.task_sandbox(&task.repo, &sibling.worktree, &sibling.run_dir)
                .as_ref(),
            cancel,
        )
        .await;
        if let Err(error) = setup {
            sibling.cleanup().await;
            return Err(crate::git::GitError(error));
        }
        Ok(sibling)
    }
    pub(super) async fn cleanup(&self) {
        let (repo, path, branch) = (
            self.repo.clone(),
            self.worktree.clone(),
            self.branch.clone(),
        );
        let _ = tokio::task::spawn_blocking(move || {
            crate::delivery::git::recovery::discard_worktree(&repo, &path, &branch)
        })
        .await;
    }
}
