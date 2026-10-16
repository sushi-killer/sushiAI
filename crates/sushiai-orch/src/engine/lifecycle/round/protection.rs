use super::{LoopExit, LoopProgress};
use crate::engine::decision::completed_implement_attempts;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::protected::prepare::needs_prepare;
use crate::engine::protected::prepare::prepare_tests;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::engine::verification::base::checks::start_final_base_jobs;
use crate::policy::routing::Tier;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    let pending_amend = &signals.pending_amend;
    let cancel = &signals.cancel;
    if completed_implement_attempts(task) == 0 && needs_prepare(task) {
        let wt = PathBuf::from(&task.worktree);
        if !prepare_tests(app, &task_id, task, &wt, cancel).await {
            mark_stopped_if_not_already(app, &task_id).await;
            permit.take();
            app.finish_task_loop(&task_id);
            return Err(LoopExit::Return);
        }
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    let dependencies_before = task.depends_on.clone();
    if let Err(error) = apply_pending_amendment(app, task, pending_amend, cancel).await {
        eprintln!("orch: applying amendment to {} failed: {error}", task.id);
        permit.take();
        app.finish_task_loop(&task.id);
        return Err(LoopExit::Return);
    }
    if task.depends_on != dependencies_before && completed_implement_attempts(task) == 0 {
        return Err(LoopExit::Retry);
    }

    // A final check that already fails on the base is looked up after
    // the attempt, from the cache or the job started here: the first
    // attempt does not wait for it.
    if completed_implement_attempts(task) == 0 && progress.parent_failure.is_none() {
        start_final_base_jobs(app, task);
    }

    if completed_implement_attempts(task) == 0 && !task.pr_check_repair_due() {
        match task.planned_tier {
            Some(tier) => {
                task.select_tier(tier, None);
            }
            None => {
                task.select_tier(Tier::Standard, Some("no planner tier".to_string()));
            }
        }
    }

    Ok(())
}
