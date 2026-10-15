use super::{LoopExit, LoopProgress};
use crate::clock::now_ms;
use crate::engine::decision::record_failure;
use crate::engine::lifecycle::base_wait::carry_after_base_moves;
use crate::engine::lifecycle::base_wait::BaseRetry;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::rate_limit::counted_attempts;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::Wait;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn retry(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    // The gates are passed: a wait on dependencies or a subtask slot is over.
    if matches!(
        task.queue.wait,
        Some(Wait::Dependency { .. } | Wait::SubtaskSlot { .. })
    ) {
        task.set_wait(None);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }

    // An answered retry survives a daemon restart without another
    // attempt or another question on the same base.
    let checks: Vec<String> = task
        .verify
        .iter()
        .chain(&task.final_verify)
        .cloned()
        .collect();
    if base_retry_requested(task, &checks, &task.base_sha) {
        let worktree = PathBuf::from(&task.worktree);
        match carry_after_base_moves(
            app,
            task,
            &worktree,
            &mut QuestionSlot::new(signals, &mut None),
        )
        .await
        {
            BaseRetry::Ready => {}
            BaseRetry::Changed => return Err(LoopExit::Retry),
            BaseRetry::Conflict(detail) => {
                if let Some(idx) = task
                    .attempts
                    .iter()
                    .rposition(|a| a.stage == Stage::Implement)
                {
                    record_failure(task, idx, FailureKind::Conflict, detail);
                } else {
                    progress.parent_failure = Some(detail);
                }
            }
            BaseRetry::Stopped => {
                mark_stopped_if_not_already(app, &task_id).await;
                app.finish_task_loop(&task_id);
                return Err(LoopExit::Return);
            }
        }
        progress.budget = progress.budget.max(counted_attempts(task) + 1);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
    }

    Ok(())
}
