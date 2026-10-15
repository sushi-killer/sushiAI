use super::resolution::{Handoff, ResolutionReceipt};
use crate::engine::delivery::pr::carry::PrCarry;
use crate::engine::lifecycle::carry::{carry_onto_newest_base, BaseCarry};
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::delivery::PrPlan;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Waits for the finished loop of `id` to leave `controls`, so that starting
/// the task again is not a no-op.
pub(super) async fn loop_gone(app: &App, id: &str) -> bool {
    for _ in 0..600 {
        if !app.controls.lock().unwrap().contains_key(id) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The PR carry conflicted: the task gets one ordinary attempt on the
/// newest base (the loop's own, with its checks and review), and opens the
/// pull request when it is done again. The conflict is carried into the task's
/// own worktree, as the loop carries one onto a moved base. A second conflict,
/// or a carry that cannot be set up, ends the task `failed` with a report and
/// one notice.
pub(in crate::engine) async fn resolve_conflict(
    app: &Arc<App>,
    id: &str,
    base: &str,
    plan: Option<PrPlan>,
) -> Result<PrCarry, String> {
    let Some(mut task) = app.store.load_task(id).ok().flatten() else {
        return Err("task not found".to_string());
    };
    let upstream = format!("origin/{base}");
    if task.queue.pr_conflict_tried {
        let reason =
            format!("the work still conflicts with {upstream} after its resolution attempt");
        return Ok(PrCarry::Failed(fail_unresolved(app, id, reason).await));
    }
    // The work conflicts, but only a task started from this very base branch
    // can resolve it in its own worktree; others keep the old outcome.
    if task.base_ref.as_deref() != Some(base) {
        return Err(format!(
            "the work conflicts with {upstream} and the task did not start from {base}, so orchd cannot resolve it"
        ));
    }
    if !loop_gone(app, id).await {
        return Err("the task loop is still running; try again later".to_string());
    }
    let repo = PathBuf::from(&task.repo);
    let tip = crate::delivery::pr::command::run_cmd(
        "git",
        &["rev-parse", &upstream],
        &repo,
        crate::delivery::pr::command::GIT_TIMEOUT,
    )
    .await?;
    let Some(queued) = queued::admit(app, &task, tip, plan)? else {
        return Ok(PrCarry::Deferred);
    };
    task = queued;
    let resolution = ResolutionReceipt::capture(&task)?;
    let setup = async {
        let recreated = app.ensure_worktree(&mut task).await?;
        let wt = PathBuf::from(&task.worktree);
        // A recreated worktree has none of its dependencies yet.
        app.ensure_worktree_setup(&task, recreated, &CancelToken::new())
            .await?;
        Ok::<_, String>(carry_onto_newest_base(app, &mut task, &wt, &CancelToken::new()).await)
    }
    .await;
    let reason = match setup {
        Ok(BaseCarry::Conflict(_)) => None,
        Ok(BaseCarry::Stale) => return handoff(app, &resolution),
        Ok(BaseCarry::Stopped) => return Ok(PrCarry::Deferred),
        Ok(BaseCarry::Ready) => Some(format!("the carry onto {upstream} left no conflict")),
        Ok(BaseCarry::Failed(reason) | BaseCarry::Skipped(reason)) | Err(reason) => Some(reason),
    };
    if let Some(reason) = reason {
        return Ok(PrCarry::Failed(fail_unresolved(app, id, reason).await));
    }
    let files = task.queue.carry_conflicts.clone();
    let line = format!(
        "PR: the work conflicts with {upstream} in {}; an ordinary attempt resolves it on the new base before the pull request opens",
        files.join(", ")
    );
    app.update_task(id, |fresh| fresh.record_decision(line))?;
    handoff(app, &resolution)
}

fn handoff(app: &App, resolution: &ResolutionReceipt) -> Result<PrCarry, String> {
    Ok(match resolution.resume(app)? {
        Handoff::Active(current) => PrCarry::Resolving(current.queue.carry_conflicts.clone()),
        Handoff::Deferred => PrCarry::Deferred,
    })
}

mod queued;

/// Ends a done task whose pull request cannot be reached: `failed`, with the
/// reason in its decisions and report, and one notice. The reason is returned.
async fn fail_unresolved(app: &Arc<App>, id: &str, reason: String) -> String {
    let line = format!("PR: not opened: {reason}; the work stays on its branch");
    let _ = app.update_task(id, |t| {
        t.record_decision(line);
        t.finish_pr();

        t.mark_failed();
    });
    if let Ok(Some(mut task)) = app.store.load_task(id) {
        app.write_report(&mut task).await;
        app.save_task_logged(&task);
        app.broadcast_task(&task);
        app.notify_task_failed(&task);
    }
    reason
}

#[cfg(test)]
mod tests;
