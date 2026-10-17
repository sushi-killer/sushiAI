use crate::clock::now_ms;
use crate::engine::state::App;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::Arc;

/// The attempt's loop was cancelled after its run: interrupted, never a
/// failure. A daemon shutdown during review or verify leaves a finished (or
/// still running) implement attempt `running` instead, and the next daemon
/// resumes into review and verify.
pub(in crate::engine) fn interrupt_attempt(app: &Arc<App>, task: &mut Task, idx: usize) {
    if app.leaves_run_going(task, idx) {
        return;
    }
    crate::task::attempts::end(
        task,
        idx,
        crate::task::attempts::AttemptEnd::Interrupted,
        now_ms(),
    );
    app.apply_cancellation(task);
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
}

pub(in crate::engine) async fn mark_stopped_if_not_already(app: &Arc<App>, task_id: &str) {
    if let Ok(Some(task)) = app.store.load_task(task_id) {
        // Decided and written on the current copy, under the task lock.
        app.update_task_if(task_id, |task| {
            let live = !matches!(task.status, TaskStatus::Done | TaskStatus::Failed);
            if live {
                app.apply_cancellation(task);
                task.touch(now_ms());
            }
            live
        });
        app.advance_queue(&task.repo);
    }
}
