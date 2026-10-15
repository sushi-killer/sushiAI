use crate::delivery::pr::checkout::TempWorktree;
use crate::engine::state::App;
use crate::engine::worktrees;
use crate::harness::cancel::CancelToken;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Carries the done task's work onto the newest `origin/<base>` before it is
/// pushed, with the task's checks run again when the base moved, as landing
/// does. A branch on an old base would show newer base files (a workflow, a
/// signature) as the task's own change. The branch moves only when the carry
/// and the checks pass; the decisions are saved either way. A carry that
/// conflicts is not a dead end: see [`resolve_conflict`].
pub(super) enum CarryCheckout {
    Ready(TempWorktree, PathBuf),
    Environment(String),
}
pub(super) async fn prepare_checkout(
    app: &Arc<App>,
    task: &Task,
    old: &str,
) -> Result<CarryCheckout, String> {
    let repo = PathBuf::from(&task.repo);
    let path =
        crate::git::worktree::worktree_path(&repo, &app.home, "", &format!("pr-carry-{}", task.id));
    let temp = TempWorktree::checkout(repo.clone(), path, old.to_string(), &task.branch).await?;
    let run_dir = app
        .store
        .claim_helper_run_dir(&task.id, "pr-carry", None)
        .map_err(|e| e.to_string())?;
    // The carry reruns the task's checks here: the tree gets the same
    // environment as a task worktree (the repo's own setup) first.
    {
        let (r, p) = (repo.clone(), temp.path.clone());
        let _ =
            tokio::task::spawn_blocking(move || crate::git::bootstrap::bootstrap_worktree(&r, &p))
                .await;
        let sandbox = app.task_sandbox(&task.repo, &temp.path, &run_dir);
        if let Err(e) =
            worktrees::setup_worktree(&repo, &temp.path, sandbox.as_ref(), &CancelToken::new())
                .await
        {
            return Ok(CarryCheckout::Environment(e));
        }
    }
    Ok(CarryCheckout::Ready(temp, run_dir))
}
