use crate::clock::now_ms;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::questions::wake::wake_orchestrator_for_done;
use crate::engine::state::App;
use crate::task::{transitions, Task};
use std::sync::Arc;

pub(super) async fn complete(
    app: &Arc<App>,
    task: &mut Task,
    index: usize,
    base_ref: &str,
    slot: &mut QuestionSlot<'_>,
) {
    task.record_decision(format!(
        "Rebase: {base_ref} already contains this work; nothing to commit"
    ));
    transitions::complete(task, Some(index), transitions::Completion::Done, now_ms());
    task.touch(now_ms());
    app.write_report(task).await;
    app.release_worktree(task, "task done").await;
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    wake_orchestrator_for_done(app, task);
    slot.release();
    app.finish_task_loop(&task.id);
}
