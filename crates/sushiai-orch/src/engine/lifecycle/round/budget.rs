use super::{LoopExit, LoopProgress};
use crate::engine::cost::wait_while_over_budget;
use crate::engine::decision::completed_implement_attempts;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::healing::preflight::start_base_checks;
use std::sync::Arc;

pub(super) async fn wait(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    let cancel = &signals.cancel;
    // Every check command runs on the base once, in the background:
    // attempt 1 starts at once and the job holds no slot. A command the
    // repository cannot run is rewritten, one broken for an unrelated
    // reason stops gating; the loop applies the ruling where it is
    // needed (a failing verify command, the gate before finalVerify).
    if completed_implement_attempts(task) == 0
        && !task.brief_check.feasibility_done
        && progress.parent_failure.is_none()
    {
        start_base_checks(app, task, cancel);
    }

    match wait_while_over_budget(
        app,
        task,
        "implement",
        &mut QuestionSlot::new(signals, permit),
    )
    .await
    {
        Ok(()) => {}
        Err(WaitEnd::Changed) => {
            permit.take();
            return Err(LoopExit::Retry);
        }
        Err(WaitEnd::Stopped) => {
            permit.take();
            app.finish_task_loop(&task_id);
            return Err(LoopExit::Return);
        }
    }

    Ok(())
}
