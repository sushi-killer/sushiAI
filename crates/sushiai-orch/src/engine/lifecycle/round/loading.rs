use super::{LoopExit, LoopProgress};
use crate::clock::now_ms;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::TaskStatus;
use std::sync::Arc;

pub(super) async fn load(
    app: &Arc<App>,
    task_id: &str,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
) -> Result<crate::task::Task, LoopExit> {
    let cancel = &signals.cancel;
    if cancel.is_cancelled() {
        mark_stopped_if_not_already(app, task_id).await;
        app.finish_task_loop(task_id);
        return Err(LoopExit::Return);
    }

    let mut task = match app.store.load_task(task_id) {
        Ok(Some(t)) => t,
        _ => {
            app.finish_task_loop(task_id);
            return Err(LoopExit::Return);
        }
    };
    if task.queue.park.is_some() {
        app.finish_task_loop(task_id);
        return Err(LoopExit::Return);
    }
    if task.queue.paused_until.is_some() {
        match crate::engine::lifecycle::limit_pause::wait(app, &mut task, signals).await {
            Ok(crate::engine::lifecycle::limit_pause::PauseWait::Ready) => {}
            Ok(crate::engine::lifecycle::limit_pause::PauseWait::Stopped) => {
                mark_stopped_if_not_already(app, task_id).await;
                app.finish_task_loop(task_id);
                return Err(LoopExit::Return);
            }
            Err(error) => {
                eprintln!("orch: resuming paused task {task_id} failed: {error}");
                app.finish_task_loop(task_id);
                return Err(LoopExit::Return);
            }
        }
    }
    task.settle_carry_conflicts();
    if !progress.budget_loaded {
        progress.budget_loaded = true;
        if let Some(n) = task.variant().max_attempts {
            progress.budget = n;
        }
    }

    // A finished task started again is no longer finished.
    if task.metrics.is_some() && !matches!(task.status, TaskStatus::Done | TaskStatus::Failed) {
        task.record_metrics(None);
        task.touch(now_ms());
        app.save_task_logged(&task);
        app.broadcast_task(&task);
    }

    Ok(task)
}
