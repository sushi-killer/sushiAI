use crate::clock::now_ms;
use crate::engine::decision::implement_attempt_count;
use crate::engine::graph::base::parent_base_preflight;
use crate::engine::lifecycle::interruption::mark_stopped_if_not_already;
use crate::engine::protected::{needs_prepare, prepare_tests};
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::harness::cancel::CancelToken;
use crate::task::graph::{own_waits_state, waits_state, Waits};
use crate::task::{Task, TaskStatus};
use std::{path::PathBuf, sync::Arc};

pub(in crate::engine::graph) fn dependencies_ready(
    app: &Arc<App>,
    task: &mut Task,
    all: &[Task],
) -> bool {
    match own_waits_state(task, all) {
        Waits::Ready | Waits::Nothing => {}
        Waits::Ended(ended) => {
            app.ask_dependency_question(task, all, &ended);
            return false;
        }
        Waits::Pending => {
            if task.status != TaskStatus::Queued {
                task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
                task.touch(now_ms());
                app.save_task_logged(task);
                app.broadcast_task(task);
            }
            return false;
        }
    }
    true
}

pub(in crate::engine::graph) async fn prepare_parent(
    app: &Arc<App>,
    task: &mut Task,
    all: &[Task],
    signals: &TaskSignals,
) -> bool {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let cancel = &signals.cancel;
    let pending_answer = &signals.pending_answer;
    let pending_amend = &signals.pending_amend;
    // Before the first child starts: the parent's checks judge the base, not
    // the children's work. Idempotent across relaunches.
    if needs_prepare(task) {
        let wt = PathBuf::from(&task.worktree);
        if !prepare_tests(app, task_id, task, &wt, cancel).await {
            mark_stopped_if_not_already(app, task_id).await;
            return false;
        }
        app.save_task_logged(task);
        app.broadcast_task(task);
    }
    // The parent's own commands, run on the base while no child has started.
    let none_started = all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
        .all(|t| {
            matches!(t.status, TaskStatus::Drafting | TaskStatus::Queued)
                && implement_attempt_count(t) == 0
        });
    let dependencies_before = task.depends_on.clone();
    if none_started
        && !parent_base_preflight(app, task, pending_answer, pending_amend, cancel).await
    {
        return false;
    }
    apply_pending_amendment(app, task, pending_amend, cancel).await;
    if task.depends_on != dependencies_before {
        task.resume_phase(TaskStatus::Queued, crate::clock::now_ms());
        task.touch(now_ms());
        app.save_task_logged(task);
        app.broadcast_task(task);
        return false;
    }
    true
}

pub(in crate::engine::graph) fn start_children(
    app: &Arc<App>,
    task: &mut Task,
    all: Vec<Task>,
    cancel: &CancelToken,
) -> Vec<Task> {
    let task_id = task.id.clone();
    let task_id = task_id.as_str();
    let restarted = task.queue.restart_children;
    task.record_restart_children(false);
    for child in all
        .iter()
        .filter(|t| t.parent.as_deref() == Some(task_id) && !t.archived)
    {
        if cancel.is_cancelled() {
            break;
        }
        match child.status {
            // A part whose dependencies have not landed is not started (so
            // not planned) yet; the graph comes back to it when they do.
            TaskStatus::Drafting | TaskStatus::Queued if child.queue.park.is_none() => {
                if matches!(waits_state(child, &all), Waits::Ready | Waits::Nothing) {
                    app.spawn_child_loop(&all, child);
                } else {
                    app.note_dependency_wait(child, &all);
                }
            }
            TaskStatus::Stopped | TaskStatus::Failed if restarted => {
                // Queued before its loop runs, so this parent never reads
                // it as ended.
                let mut child = child.clone();
                let mut cancellations = app.cancellations.lock().unwrap();
                if let Some(end) = cancellations.get_mut(&child.id) {
                    end.restart = true;
                }
                child.queue_child_restart(child.queue.restart_children, now_ms());
                app.save_task_logged(&child);
                app.broadcast_task(&child);
                drop(cancellations);
                app.spawn_child_loop(&all, &child);
            }
            _ => {}
        }
    }

    if restarted {
        app.repo_tasks(&task.repo)
    } else {
        all
    }
}

pub(in crate::engine::graph) fn children_ready(
    app: &Arc<App>,
    task: &mut Task,
    all: &[Task],
) -> bool {
    if !matches!(waits_state(task, all), Waits::Ready | Waits::Nothing) {
        if task.status != TaskStatus::Running {
            task.resume_phase(TaskStatus::Running, crate::clock::now_ms());
            task.touch(now_ms());
            app.save_task_logged(task);
            app.broadcast_task(task);
        }
        return false;
    }
    true
}
