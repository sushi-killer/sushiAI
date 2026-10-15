use crate::clock::now_ms;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::checks::admission::CheckAdmission;
use crate::task::Task;

pub(in crate::engine) enum DeliveryGate {
    Ready,
    Recheck,
    Stopped,
}

/// Admission and amendment handoff share task_lock: a queued edit either
/// reaches another check round or sees the persisted landing phase.
pub(in crate::engine) fn begin_delivery(
    app: &App,
    task: &mut Task,
    cancel: &CancelToken,
    expected: &CheckAdmission,
) -> Result<DeliveryGate, String> {
    {
        let lock = app.task_lock(&task.id);
        let _guard = lock.lock().unwrap();
        let fresh = app
            .store
            .load_task(&task.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if cancel.is_cancelled() || !fresh.delivery_allowed() {
            return Ok(DeliveryGate::Stopped);
        }
        if !expected.matches(&fresh) || !expected.matches(task) {
            return Ok(DeliveryGate::Recheck);
        }
        let pending = app
            .controls
            .lock()
            .unwrap()
            .get(&task.id)
            .is_some_and(|control| control.signals.pending_amend.lock().unwrap().is_some());
        if pending || fresh.pending_amendment.is_some() {
            app.store
                .save_task(task)
                .map_err(|error| error.to_string())?;
            return Ok(DeliveryGate::Recheck);
        }
        if !task.begin_delivery(now_ms()) {
            return Ok(DeliveryGate::Stopped);
        }
        app.store
            .save_task(task)
            .map_err(|error| error.to_string())?;
    }
    app.broadcast_task(task);
    Ok(DeliveryGate::Ready)
}

/// Publishes an async carry only while its original plan still owns the task.
pub(in crate::engine) fn publish_carry(
    app: &App,
    task: &Task,
    before: &Task,
    cancel: &CancelToken,
) -> Result<DeliveryGate, String> {
    let lock = app.task_lock(&task.id);
    let _guard = lock.lock().unwrap();
    let fresh = app
        .store
        .load_task(&task.id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "task not found".to_string())?;
    if cancel.is_cancelled() || !fresh.carry_allowed(before) {
        return Ok(DeliveryGate::Stopped);
    }
    if !CheckAdmission::capture(before).matches(&fresh) {
        return Ok(DeliveryGate::Recheck);
    }
    let controls = app.controls.lock().unwrap();
    let mut pending = controls
        .get(&task.id)
        .map(|control| control.signals.pending_amend.lock().unwrap());
    let expected = fresh.pending_amendment.clone();
    let mut published = task.clone();
    published.adopt_pending_amendment(&fresh);
    if published.carry_pending_amendment(&fresh).is_err() {
        return Ok(DeliveryGate::Recheck);
    }
    app.store
        .save_task_with_pending(&published, expected.as_ref())
        .map_err(|error| error.to_string())?;
    if let Some(slot) = pending.as_deref_mut() {
        *slot = published.pending_amendment.clone();
    }
    Ok(DeliveryGate::Ready)
}

#[cfg(test)]
#[path = "delivery_gate/tests.rs"]
mod tests;
