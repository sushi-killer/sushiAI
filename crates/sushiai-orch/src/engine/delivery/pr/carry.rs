use super::{
    carried_work::{carry_work, CarriedWork},
    checkout::{prepare_checkout, CarryCheckout},
};
use crate::engine::delivery::pr::conflict::resolve_conflict;
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
/// conflicts is not a dead end: see [`resolve_conflict`].
pub(in crate::engine) async fn carry_for_pr(
    app: &Arc<App>,
    task: &Task,
    plan: Option<PrPlan>,
) -> Result<PrCarry, String> {
    let repo = PathBuf::from(&task.repo);
    let default = {
        let repo = repo.clone();
        tokio::task::spawn_blocking(move || crate::git::branches::default_branch(&repo))
            .await
            .map_err(|e| e.to_string())?
    };
    let base = crate::delivery::pr::branch::pr_base(task, &repo, default)
        .await
        .map_err(|e| e.to_string())?;
    crate::delivery::pr::command::run_cmd(
        "git",
        &["fetch", "origin", &base],
        &repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await?;
    let (upstream, local) = (
        format!("origin/{base}"),
        format!("refs/heads/{}", task.branch),
    );
    let contains = crate::delivery::pr::command::run_cmd(
        "git",
        &["merge-base", "--is-ancestor", &upstream, &local],
        &repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await
    .is_ok();
    if contains {
        return Ok(PrCarry::Ready);
    }
    let old = crate::delivery::pr::command::run_cmd(
        "git",
        &["rev-parse", &local],
        &repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await?;
    let (temp, run_dir) = match prepare_checkout(app, task, &old).await? {
        CarryCheckout::Ready(temp, run_dir) => (temp, run_dir),
        CarryCheckout::Environment(e) => return Ok(PrCarry::Environment(e)),
    };
    let CarriedWork {
        work,
        before,
        committed,
        conflicted,
        contained,
    } = carry_work(app, task, &temp, &run_dir, &upstream).await?;
    let moved = match &committed {
        Ok(sha) => {
            crate::delivery::pr::command::run_cmd(
                "git",
                &["update-ref", &local, sha, &old],
                &repo,
                crate::delivery::pr::command::GIT_TIMEOUT,
            )
            .await
        }
        Err(e) => Err(e.clone()),
    };
    drop(temp);
    // On `contained` the carry's own line is the one decision line.
    let lines = work.decisions[before..].to_vec();
    let ok = moved.is_ok();
    let _ = app.update_task(&task.id, move |t| {
        t.record_decisions(lines);
        if ok {
            crate::task::transitions::move_base(t, work.base_sha.clone());
            crate::task::checks::edits::adopt_planner_tests(t, work.planner_tests.clone());
        }
    });
    if contained {
        return Ok(PrCarry::Contained);
    }
    if conflicted {
        return resolve_conflict(app, &task.id, &base, plan).await;
    }
    moved.map(|_| PrCarry::Ready)
}

/// What the carry of a done task onto the newest base came to.
pub(in crate::engine) enum PrCarry {
    /// The branch holds the newest base; the pull request can be pushed.
    Ready,
    /// The newest base already holds the work: there is nothing to open.
    Contained,
    /// The work conflicts with the newest base and the task went back for its
    /// one resolution attempt; it opens the pull request itself when done.
    Resolving(Vec<String>),
    /// The task ended `failed`, with a report and a notice; the reason.
    Failed(String),
    /// The repo's setup failed in the carry worktree: an environment
    /// problem, not a verdict on the work. Nothing moved.
    Environment(String),
    /// The task changed or stopped while carrying; its current state owns the next action.
    Deferred,
}
