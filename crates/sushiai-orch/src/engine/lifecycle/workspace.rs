use crate::clock::now_ms;
use crate::engine::ladder;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use std::sync::Arc;

pub(in crate::engine) async fn restore(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) -> Option<bool> {
    let mut tries = 0;
    loop {
        match app.ensure_worktree(task).await {
            Ok(true) => {
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
                return Some(true);
            }
            Ok(false) => return Some(false),
            Err(error) if tries < ladder::SETUP_RETRIES && !cancel.is_cancelled() => {
                tries += 1;
                task.record_decision(format!(
                    "Worktree: {error}; trying again ({tries}/{})",
                    ladder::SETUP_RETRIES
                ));
                app.save_task_logged(task);
                if !ladder::sleep_unless_cancelled(ladder::setup_retry_delay(tries), cancel).await {
                    mark_stopped_if_not_already(app, &task.id).await;
                    app.finish_task_loop(&task.id);
                    return None;
                }
            }
            Err(error) => {
                fail(app, task, format!("Worktree: {error}"));
                return None;
            }
        }
    }
}

pub(in crate::engine) async fn setup(
    app: &Arc<App>,
    task: &mut Task,
    recreated: bool,
    cancel: &CancelToken,
) -> bool {
    let mut tries = 0;
    loop {
        let Err(error) = app.ensure_worktree_setup(task, recreated, cancel).await else {
            return true;
        };
        if cancel.is_cancelled() {
            mark_stopped_if_not_already(app, &task.id).await;
            app.finish_task_loop(&task.id);
            return false;
        }
        if tries >= ladder::SETUP_RETRIES {
            fail(app, task, format!("Worktree setup: {error}"));
            return false;
        }
        tries += 1;
        task.record_decision(format!(
            "Worktree setup: {error}; trying again ({tries}/{})",
            ladder::SETUP_RETRIES
        ));
        app.save_task_logged(task);
        if !ladder::sleep_unless_cancelled(ladder::setup_retry_delay(tries), cancel).await {
            mark_stopped_if_not_already(app, &task.id).await;
            app.finish_task_loop(&task.id);
            return false;
        }
    }
}

fn fail(app: &App, task: &mut Task, detail: String) {
    task.record_decision(detail);
    task.mark_failed();
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    app.finish_task_loop(&task.id);
}
