use crate::delivery::pr::publication::PreparedWrite;
use crate::engine::state::App;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{Task, TaskStatus};

/// Authority accepted by the carry, retained through the final PR preparation.
pub(in crate::engine) struct PrAdmission {
    checks: CheckAdmission,
    branch: String,
    base_ref: Option<String>,
}

pub(super) enum Admission<T> {
    Ready(T),
    Changed(Box<Task>),
    Deferred,
}

impl PrAdmission {
    pub(super) fn capture(task: &Task) -> Self {
        Self {
            checks: CheckAdmission::capture(task),
            branch: task.branch.clone(),
            base_ref: task.base_ref.clone(),
        }
    }

    pub(super) fn dispatch<W: PreparedWrite>(
        &self,
        app: &App,
        id: &str,
        write: W,
    ) -> Result<Admission<W::Running>, String> {
        let lock = app.task_lock(id);
        let _guard = lock.lock().unwrap();
        let fresh = app
            .store
            .load_task(id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        let blocked = app.controls.lock().unwrap().get(id).is_some_and(|control| {
            control.signals.cancel.is_cancelled()
                || control.signals.pending_amend.lock().unwrap().is_some()
        });
        if fresh.status != TaskStatus::Done
            || fresh.archived
            || fresh.queue.park.is_some()
            || fresh.pending_amendment.is_some()
            || fresh.landed_sha.is_some()
            || blocked
        {
            return Ok(Admission::Deferred);
        }
        if !self.checks.matches(&fresh)
            || self.branch != fresh.branch
            || self.base_ref != fresh.base_ref
        {
            return Ok(Admission::Changed(Box::new(fresh)));
        }
        Ok(Admission::Ready(write.dispatch()?))
    }
}
