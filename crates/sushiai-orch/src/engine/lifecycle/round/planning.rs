use super::{LoopExit, LoopProgress};
use crate::clock::now_ms;
use crate::engine::fresh_base;
use crate::engine::planning::records::needs_planning;
use crate::engine::planning::run::run_plan_stage;
use crate::engine::planning::run::PlanOutcome;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::task::TaskStatus;
use std::sync::Arc;

pub(super) async fn plan(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    let pending_amend = &signals.pending_amend;
    let cancel = &signals.cancel;
    if needs_planning(task) {
        // A task planned late (a backlog request) plans on the base as it
        // is now, when nothing has touched its worktree yet.
        // A subtask keeps the parent-head move done before attempt 1.
        if task.attempts.is_empty() && task.parent.is_none() {
            fresh_base::plan_on_newest_base(task).await;
        }
        if task.status == TaskStatus::Queued {
            task.resume_phase(TaskStatus::Drafting, task.updated_at);
        }
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        let planned = run_plan_stage(
            app,
            &task_id,
            signals,
            permit,
            progress.auto_start_after_plan,
        )
        .await;
        if let Ok(Some(mut planned_task)) = app.store.load_task(&task_id) {
            if let Err(error) =
                apply_pending_amendment(app, &mut planned_task, pending_amend, cancel).await
            {
                eprintln!(
                    "orch: applying amendment to {} failed: {error}",
                    planned_task.id
                );
                permit.take();
                app.finish_task_loop(&planned_task.id);
                return Err(LoopExit::Return);
            }
        }
        match planned {
            PlanOutcome::Proceed => {
                permit.take();
                return Err(LoopExit::Retry);
            }
            PlanOutcome::StoppedForReview | PlanOutcome::Ended => {
                permit.take();
                app.finish_task_loop(&task_id);
                return Err(LoopExit::Return);
            }
        }
    }

    Ok(())
}
