use super::jobs::BaseCheckJob;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::sandbox::TaskSandbox;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) struct BaseCheckout {
    scratch: PathBuf,
    pub(super) worktree: PathBuf,
    pub(super) sandbox: Option<TaskSandbox>,
}
impl BaseCheckout {
    pub(super) async fn prepare(
        app: &Arc<App>,
        job: &BaseCheckJob,
        cancel: &CancelToken,
    ) -> (Self, Result<(), String>) {
        let scratch = app
            .data_dir
            .join("base-runs")
            .join(uuid::Uuid::new_v4().to_string());
        let wt = scratch.join("base");
        let (repo_path, sha, scratch2, wt2) = (
            PathBuf::from(&job.repo),
            job.base_sha.clone(),
            scratch.clone(),
            wt.clone(),
        );
        let setup = tokio::task::spawn_blocking(move || -> Result<(), String> {
            std::fs::create_dir_all(&scratch2).map_err(|e| e.to_string())?;
            crate::git::worktree::add_detached_worktree(&repo_path, &wt2, &sha)
                .map_err(|e| e.to_string())?;
            crate::git::bootstrap::bootstrap_worktree(&repo_path, &wt2).map_err(|e| e.to_string())
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let _ = std::fs::create_dir_all(&job.run_dir);
        let sandbox = app.task_sandbox(&job.repo, &wt, &job.run_dir);
        let setup = match setup {
            Ok(()) => {
                crate::engine::worktrees::setup_worktree(
                    Path::new(&job.repo),
                    &wt,
                    sandbox.as_ref(),
                    cancel,
                )
                .await
            }
            err => err,
        };
        (
            Self {
                scratch,
                worktree: wt,
                sandbox,
            },
            setup,
        )
    }
    pub(super) async fn remove(self, repo: &str) {
        let (repo_path, wt3) = (PathBuf::from(repo), self.worktree);
        let _ = tokio::task::spawn_blocking(move || {
            crate::git::worktree::remove_worktree(&repo_path, &wt3);
            let _ = std::fs::remove_dir_all(&self.scratch);
        })
        .await;
    }
}
