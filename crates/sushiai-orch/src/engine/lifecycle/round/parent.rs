use super::{LoopExit, LoopProgress};
use crate::engine::decision::completed_implement_attempts;
use crate::engine::graph::parent::run_parent;
use crate::engine::graph::parent::ParentEnd;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::graph::is_parent;
use std::sync::Arc;

pub(super) async fn admit(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    // A child may have been attached while this task waited for a slot:
    // it must never implement the whole request itself then.
    if completed_implement_attempts(task) == 0
        && progress.parent_failure.is_none()
        && !task.pr_check_repair_due()
    {
        let all = app.repo_tasks(&task.repo);
        if is_parent(task, &all) {
            permit.take();
            match run_parent(app, &task_id, signals).await {
                ParentEnd::Finished => {
                    app.finish_task_loop(&task_id);
                    return Err(LoopExit::Return);
                }
                ParentEnd::Attempt { failure, budget } => {
                    progress.parent_failure = Some(failure);
                    progress.budget = budget.unwrap_or(progress.budget);
                    return Err(LoopExit::Retry);
                }
            }
        }
    }

    Ok(())
}
