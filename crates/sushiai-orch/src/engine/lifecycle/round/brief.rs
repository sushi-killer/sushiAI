use super::{LoopExit, LoopProgress};
use crate::engine::brief_check::refresh::refresh_after_dependencies;
use crate::engine::brief_check::run::check_brief;
use crate::engine::brief_check::run::BriefAction;
use crate::engine::decision::completed_implement_attempts;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::healing::preflight::start_base_checks;
use std::sync::Arc;

pub(super) async fn prepare(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    let cancel = &signals.cancel;
    // A parent-drafted child may have been drafted against files its
    // dependencies then changed: one cheap refresh of its commands and
    // paths, before its brief is checked.
    if completed_implement_attempts(task) == 0
        && progress.parent_failure.is_none()
        && task.planned_by_parent
        && !task.brief_check.refreshed
    {
        match refresh_after_dependencies(app, &task_id, cancel).await {
            Ok(()) => {}
            Err(WaitEnd::Changed) => {
                permit.take();
                return Err(LoopExit::Retry);
            }
            Err(WaitEnd::Stopped) => {
                permit.take();
                mark_stopped_if_not_already(app, &task_id).await;
                app.finish_task_loop(&task_id);
                return Err(LoopExit::Return);
            }
        }
        if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
            *task = reloaded;
        }
    }

    // Explicit criteria are checked once, before the first attempt; a
    // planned brief was already checked in the plan stage. The base
    // checks start beside the brief check, not after it.
    if completed_implement_attempts(task) == 0
        && !task.brief_check.done
        && progress.parent_failure.is_none()
    {
        start_base_checks(app, task, cancel);
        if let BriefAction::Cancelled = check_brief(app, &task_id, false, cancel).await {
            permit.take();
            mark_stopped_if_not_already(app, &task_id).await;
            app.finish_task_loop(&task_id);
            return Err(LoopExit::Return);
        }
        if let Ok(Some(reloaded)) = app.store.load_task(&task_id) {
            *task = reloaded;
        }
    }

    Ok(())
}
