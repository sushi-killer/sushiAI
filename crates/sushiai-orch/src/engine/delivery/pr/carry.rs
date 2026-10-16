use self::round::{carry_round, RoundOutcome};
use super::admission::PrAdmission;
use crate::engine::state::App;
use crate::task::delivery::PrPlan;
use crate::task::state::Task;
use std::path::PathBuf;
use std::sync::Arc;

/// Carries the done task's work onto the newest `origin/<base>` before it is
/// pushed, with the task's checks run again when the base moved, as landing
/// does. A branch on an old base would show newer base files (a workflow, a
/// signature) as the task's own change. The branch moves only when the carry
/// and the checks pass; the decisions are saved either way. A carry that
/// conflicts is not a dead end: see [`super::conflict::resolve_conflict`].
pub(in crate::engine) async fn carry_for_pr(
    app: &Arc<App>,
    task: &Task,
    plan: Option<PrPlan>,
) -> Result<PrCarry, String> {
    let mut current = task.clone();
    loop {
        let repo = PathBuf::from(&current.repo);
        let default = {
            let repo = repo.clone();
            tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
                .await
                .map_err(|e| e.to_string())?
        };
        let base = crate::delivery::pr::branch::pr_base(&current, &repo, default)
            .await
            .map_err(|e| e.to_string())?;
        crate::delivery::pr::command::run_cmd(
            "git",
            &["fetch", "origin", &base],
            &repo,
            crate::delivery::pr::command::GIT_TIMEOUT,
        )
        .await?;
        match carry_round(app, &current, &base, plan.clone()).await? {
            RoundOutcome::Finished(result) => return Ok(result),
            RoundOutcome::Changed(fresh) => current = *fresh,
        }
    }
}

/// What the carry of a done task onto the newest base came to.
pub(in crate::engine) enum PrCarry {
    /// The branch holds the newest base; the pull request can be pushed.
    Ready(Box<PrAdmission>),
    /// The newest base already holds the work: there is nothing to open.
    Contained,
    /// The work conflicts with the newest base and the task went back for its
    /// one resolution attempt; it opens the pull request itself when done.
    Resolving(Vec<String>),
    /// A check repair step is queued, or its repeated cause is waiting for the owner.
    ChecksPending,
    /// The task ended `failed`, with a report and a notice; the reason.
    Failed(String),
    /// The repo's setup failed in the carry worktree: an environment
    /// problem, not a verdict on the work. Nothing moved.
    Environment(String),
    /// The task changed or stopped while carrying; its current state owns the next action.
    Deferred,
}

mod round;

#[cfg(test)]
mod tests;
