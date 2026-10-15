use crate::clock::now_ms;
use crate::engine::state::App;
use crate::policy::routing::Route;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::{Stage, Task, TaskStatus};

pub(in crate::engine) fn start(
    app: &App,
    task: &mut Task,
    route: &Route,
    reason: &str,
    conflicts: &[String],
) -> Result<Option<usize>, String> {
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let fresh = app
        .store
        .load_task(&task.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    let controls = app.controls.lock().unwrap();
    let Some(control) = controls.get(&task.id) else {
        return Ok(None);
    };
    if control.signals.cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || !matches!(fresh.status, TaskStatus::Queued | TaskStatus::Running)
        || fresh.status != task.status
        || !CheckSnapshot::capture(task).matches(&fresh)
        || control
            .signals
            .pending_amend
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|edit| edit.depends_on.is_some())
    {
        return Ok(None);
    }
    let now = now_ms();
    let attempt_n = crate::task::attempts::next_number(task);
    let index = crate::task::attempts::start(task, Stage::Implement, route, reason, now);
    if !conflicts.is_empty() {
        task.record_conflict_attempt(attempt_n);
        task.record_carry_conflicts(Vec::new());
    }
    crate::task::checks::results::refresh_criteria_results(task, index);
    task.resume_phase(TaskStatus::Running, now);
    task.touch(now);
    app.store.save_task(task).map_err(|e| e.to_string())?;
    app.broadcast_task(task);
    Ok(Some(index))
}

#[cfg(test)]
mod tests;
