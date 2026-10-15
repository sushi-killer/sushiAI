use super::{LoopExit, LoopProgress};
use crate::clock::now_ms;
use crate::engine::decision::completed_implement_attempts;
use crate::engine::questions::parking::{QuestionSlot, WaitEnd};
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::TaskStatus;
use std::sync::Arc;

pub(super) async fn check(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    // A criterion the task has no tool for is put to the owner before an
    // attempt is spent on it.
    if completed_implement_attempts(task) == 0
        && progress.parent_failure.is_none()
        && !task.brief_check.missing_tool_asked
    {
        if let Some(missing) = task.brief_check.missing_tool.clone() {
            // Only enabling a connected tool is the owner's to allow;
            // without one the task goes on without the capability.
            if missing.tool_id.is_none() {
                task.record_decision(format!(
                    "Orchestrator: no tool exists for {}; the task goes on without it",
                    missing.capability
                ));
            } else {
                task.ask(
                    crate::engine::permissions::capabilities::missing_tool_question(task, &missing),
                    task.updated_at,
                );
                task.resume_phase(TaskStatus::Waiting, task.updated_at);
                task.touch(now_ms());
                let answer = match QuestionSlot::new(signals, permit).answer(app, task).await {
                    Ok(answer) => answer,
                    Err(WaitEnd::Changed) => {
                        permit.take();
                        return Err(LoopExit::Retry);
                    }
                    Err(WaitEnd::Stopped) => {
                        permit.take();
                        app.finish_task_loop(&task_id);
                        return Err(LoopExit::Return);
                    }
                };
                crate::engine::permissions::capabilities::apply_missing_answer(
                    app, task, &missing, &answer,
                );
            }
            task.mark_missing_tool_asked();
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
            permit.take();
            return Err(LoopExit::Retry);
        }
    }

    Ok(())
}
