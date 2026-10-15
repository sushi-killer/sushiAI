use crate::clock::now_ms;
use crate::engine::questions::parking::WaitEnd;
use crate::engine::state::App;
use crate::harness::cancel::CancelToken;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{Task, TaskStatus};

/// Admission content for an asynchronous result that may change the check contract.
pub(in crate::engine) struct ContractInput {
    snapshot: CheckAdmission,
    phase: TaskStatus,
    question: Option<serde_json::Value>,
}

pub(in crate::engine) enum Publication<T> {
    Applied(T),
    Deferred(WaitEnd),
}

impl<T> Publication<T> {
    pub(in crate::engine) fn into_result(self) -> Result<T, WaitEnd> {
        match self {
            Self::Applied(value) => Ok(value),
            Self::Deferred(end) => Err(end),
        }
    }
}

impl ContractInput {
    pub(in crate::engine) fn capture(task: &Task) -> Self {
        Self {
            snapshot: CheckAdmission::capture(task),
            phase: task.status,
            question: task
                .question
                .as_ref()
                .and_then(|q| serde_json::to_value(q).ok()),
        }
    }

    pub(in crate::engine) fn publish<T>(
        &self,
        app: &App,
        observed: &Task,
        cancel: &CancelToken,
        mutation: impl FnOnce(&mut Task) -> Result<T, String>,
    ) -> Result<Publication<T>, String> {
        let lock = app.task_lock(&observed.id);
        let guard = lock.lock().unwrap();
        let mut fresh = app
            .store
            .load_task(&observed.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let pending_slot = app
            .controls
            .lock()
            .unwrap()
            .get(&observed.id)
            .map(|control| control.signals.pending_amend.clone());
        let pending = pending_slot.as_ref().map(|slot| slot.lock().unwrap());
        if cancel.is_cancelled()
            || fresh.archived
            || fresh.queue.park.is_some()
            || fresh.queue.stopped_explicitly
            || matches!(
                fresh.status,
                TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
            )
        {
            return Ok(Publication::Deferred(WaitEnd::Stopped));
        }
        if fresh.pending_amendment.is_some()
            || pending.as_deref().is_some_and(|edit| edit.is_some())
            || fresh.queue.paused_until.is_some()
            || fresh.status != self.phase
            || !self.snapshot.matches(&fresh)
            || !self.snapshot.matches(observed)
            || fresh
                .question
                .as_ref()
                .and_then(|q| serde_json::to_value(q).ok())
                != self.question
        {
            return Ok(Publication::Deferred(WaitEnd::Changed));
        }
        let result = mutation(&mut fresh)?;
        fresh.touch(now_ms());
        app.store.save_task(&fresh).map_err(|e| e.to_string())?;
        drop(pending);
        drop(guard);
        app.broadcast_task(&fresh);
        Ok(Publication::Applied(result))
    }
}

#[cfg(test)]
pub(in crate::engine) mod tests;
