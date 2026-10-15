use crate::engine::fresh_base;
use crate::engine::state::App;
use crate::task::creation::NewTask;
use crate::task::Task;
use std::path::Path;

impl App {
    /// Creates the task's branch and worktree and saves its record, status
    /// `drafting` for a request, `queued` otherwise. Shared by `task.create`
    /// and the planner's split into subtasks.
    pub(in crate::engine) async fn create_task_record(&self, new: NewTask) -> Result<Task, String> {
        let repo_root = new.repo_root.clone();
        let title_for_branch = new.title.clone();
        let branch_opt = new.branch.clone();
        let (base, base_from) = (new.base.clone(), new.base_from);
        let fetch_timeout = self.base_fetch_timeout();
        let wt_root = self.settings.read().unwrap().worktree_root.clone();
        let home = self.home.clone();
        let created = tokio::task::spawn_blocking(move || {
            let branch = branch_opt.unwrap_or_else(|| {
                crate::git::branches::unique_branch_name(&repo_root, &title_for_branch)
            });
            let wt_path = crate::git::worktree::worktree_path(&repo_root, &home, &wt_root, &branch);
            if let Some(dir) = wt_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| crate::git::GitError(e.to_string()))?;
                crate::git::worktree::exclude_worktree_root(&repo_root, dir)
                    .map_err(|e| crate::git::GitError(e.to_string()))?;
            }
            // A named base is fetched first, so the task branches from the
            // branch as its remote has it now.
            let prepared = fresh_base::prepare_new(&repo_root, &base, base_from, fetch_timeout);
            let created = crate::git::worktree::create_worktree(
                &repo_root,
                &branch,
                &wt_path,
                &prepared.rev,
            )?;
            if let Err(e) = crate::git::bootstrap::bootstrap_worktree(&repo_root, &created.path) {
                crate::delivery::git::recovery::discard_worktree(
                    &repo_root,
                    &created.path,
                    &branch,
                );
                return Err(crate::git::GitError(e.to_string()));
            }
            Ok::<_, crate::git::GitError>((branch, created, prepared))
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        let (branch, created, prepared) = created;

        let task = crate::task::creation::create(
            new,
            crate::task::creation::Workspace {
                path: created.path.to_string_lossy().to_string(),
                branch,
                base_sha: created.base_sha,
                base_ref: prepared.base_ref,
                fresh_base: prepared.fresh,
                warnings: prepared.warnings,
            },
        );
        if let Err(e) = self.store.save_task(&task) {
            let _ = std::fs::remove_dir_all(self.store.task_dir(&task.id));
            let (repo, wt, branch) = (
                task.repo.clone(),
                task.worktree.clone(),
                task.branch.clone(),
            );
            let _ = tokio::task::spawn_blocking(move || {
                crate::delivery::git::recovery::discard_worktree(
                    Path::new(&repo),
                    Path::new(&wt),
                    &branch,
                )
            })
            .await;
            return Err(e.to_string());
        }
        Ok(task)
    }
}
