use super::checks::baseline_on_base;
use crate::clock::now_ms;
use crate::engine::answer_policy::authority::check_authority;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::amendment::PendingAmendment;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::Task;
use std::sync::{Arc, Mutex};

enum Admission {
    Empty,
    Rejected,
    Applied,
}

/// Publish the admitted contract before baseline IO lets another RPC enqueue
/// its amendment against the task's current plan.
pub(in crate::engine) async fn apply_pending_amendment(
    app: &Arc<App>,
    task: &mut Task,
    pending: &Arc<Mutex<Option<PendingAmendment>>>,
    cancel: &CancelToken,
) -> Result<(), String> {
    let admitted = admit(app, task, pending, cancel)?;
    if matches!(admitted, Admission::Empty) {
        return Ok(());
    }
    app.broadcast_task(task);
    if matches!(admitted, Admission::Rejected) {
        return Ok(());
    }
    baseline_current_plan(app, task, cancel).await;
    Ok(())
}

pub(in crate::engine) async fn baseline_current_plan(
    app: &Arc<App>,
    task: &mut Task,
    cancel: &CancelToken,
) {
    let expected = CheckSnapshot::capture(task);
    baseline_on_base(app, task, cancel).await;
    if let Err(error) = publish_baselines(app, task, &expected, cancel) {
        eprintln!(
            "orch: saving amendment baselines for {} failed: {error}",
            task.id
        );
    }
}

fn admit(
    app: &App,
    task: &mut Task,
    pending: &Mutex<Option<PendingAmendment>>,
    cancel: &CancelToken,
) -> Result<Admission, String> {
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let mut slot = pending.lock().unwrap();
    let fresh = app
        .store
        .load_task(&task.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    *slot = fresh.pending_amendment.clone();
    let Some(edit) = fresh.pending_amendment.clone() else {
        task.adopt_pending_amendment(&fresh);
        return Ok(Admission::Empty);
    };
    if cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || matches!(
            fresh.status,
            crate::task::TaskStatus::Stopped
                | crate::task::TaskStatus::Failed
                | crate::task::TaskStatus::Done
                | crate::task::TaskStatus::Landing
        )
    {
        *task = fresh;
        return Ok(Admission::Empty);
    }
    if !CheckSnapshot::capture(task).matches(&fresh) {
        *task = fresh;
    } else {
        task.adopt_pending_amendment(&fresh);
    }
    let settings = app.settings.read().unwrap().clone();
    let invalidate = edit.verify.is_some();
    let mut next = task.clone();
    let admitted = match next.settle_pending_amendment(&check_authority(&settings, task), now_ms())
    {
        crate::task::amendment::storage::Settlement::Applied => Admission::Applied,
        crate::task::amendment::storage::Settlement::Rejected => Admission::Rejected,
        crate::task::amendment::storage::Settlement::Empty => Admission::Empty,
    };
    app.store
        .save_task_with_pending(&next, Some(&edit))
        .map_err(|e| e.to_string())?;
    *task = next;
    slot.take();
    if invalidate && matches!(admitted, Admission::Applied) {
        app.verify_cache.lock().unwrap().remove(&task.id);
    }
    Ok(admitted)
}

fn publish_baselines(
    app: &App,
    task: &mut Task,
    expected: &CheckSnapshot,
    cancel: &CancelToken,
) -> Result<(), String> {
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let mut fresh = app
        .store
        .load_task(&task.id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    if cancel.is_cancelled()
        || fresh.archived
        || fresh.queue.park.is_some()
        || fresh.status != task.status
        || !expected.matches(&fresh)
    {
        *task = fresh;
        return Ok(());
    }
    crate::task::checks::baseline::adopt(&mut fresh, task);
    for decision in &task.decisions {
        if !fresh.decisions.contains(decision) {
            fresh.record_decision(decision.clone());
        }
    }
    app.store.save_task(&fresh).map_err(|e| e.to_string())?;
    task.adopt_pending_amendment(&fresh);
    app.broadcast_task(&fresh);
    Ok(())
}

#[cfg(test)]
mod tests;
