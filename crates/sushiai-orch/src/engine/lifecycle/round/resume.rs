use super::{LoopExit, LoopProgress};
use crate::clock::now_ms;
use crate::engine::ladder;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::lifecycle::completion::finish::ReviewedAttempt;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::task::TaskStatus;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) async fn delivery(
    app: &Arc<App>,
    task: &mut crate::task::Task,
    signals: &TaskSignals,
    progress: &mut LoopProgress,
    permit: &mut Option<crate::engine::slots::SlotPermit>,
) -> Result<(), LoopExit> {
    let task_id = task.id.clone();
    // A task that passed everything and waited to land: only the landing
    // is tried again.
    if task.status == TaskStatus::Landing && !task.attempts.is_empty() {
        let idx = task.attempts.len() - 1;
        let attempt_n = task.attempts[idx].n;
        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let worktree = PathBuf::from(&task.worktree);
        match AttemptFiles::new(idx, attempt_n, worktree.clone(), run_dir.clone())
            .finish(
                app,
                task,
                &mut progress.budget,
                &mut QuestionSlot::new(signals, permit),
            )
            .await
        {
            Tail::Continue => {
                return Err(LoopExit::Retry);
            }
            Tail::Return => return Err(LoopExit::Return),
        }
    }

    // A retired "no verdict" question: only the final checks and the
    // commit are left, and the work is not landed.
    if let Some(idx) = ladder::pending_unreviewed(task) {
        let attempt_n = task.attempts[idx].n;
        let run_dir = app.store.run_dir(&task_id, attempt_n);
        let _ = std::fs::create_dir_all(&run_dir);
        let settings = app.settings.read().unwrap().clone();
        let worktree = PathBuf::from(&task.worktree);
        crate::task::transitions::reopen(task, idx, now_ms());
        task.clear_unreviewed_attempt();
        task.resume_phase(TaskStatus::Running, task.updated_at);
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        match ReviewedAttempt::new(
            AttemptFiles::new(idx, attempt_n, worktree.clone(), run_dir.clone()),
            &settings,
            None,
        )
        .finish(
            app,
            task,
            &mut progress.budget,
            &mut QuestionSlot::new(signals, permit),
        )
        .await
        {
            Tail::Continue => {
                return Err(LoopExit::Retry);
            }
            Tail::Return => return Err(LoopExit::Return),
        }
    }

    Ok(())
}
