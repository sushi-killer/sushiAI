use crate::engine::state::App;
use crate::task::attempts::native::Settlement;
use crate::task::checks::admission::CheckAdmission;
use crate::task::{Task, TaskStatus};

pub(in crate::engine) struct NativeSettlement {
    receipt: Settlement,
    contract: CheckAdmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::engine) enum Publication {
    Current,
    Changed,
    Stopped,
}

impl NativeSettlement {
    pub(in crate::engine) fn capture(
        task: &Task,
        index: usize,
        selection_cost: f64,
        discarded: &[String],
    ) -> Result<Self, String> {
        Ok(Self {
            receipt: Settlement::capture(task, index, selection_cost, discarded)?,
            contract: CheckAdmission::capture(task),
        })
    }

    pub(in crate::engine) fn publish(
        &mut self,
        app: &App,
        task: &mut Task,
    ) -> Result<Publication, String> {
        let observed = self.receipt.observation(task)?.clone();
        let mut publication = Publication::Current;
        let saved = app.update_task_checked(&task.id, |fresh| {
            self.receipt.merge(fresh, &observed)?;
            let controls = app.controls.lock().unwrap();
            let signals = controls.get(&task.id).map(|control| &control.signals);
            let stopped = fresh.archived
                || fresh.queue.park.is_some()
                || matches!(
                    fresh.status,
                    TaskStatus::Stopped | TaskStatus::Done | TaskStatus::Failed
                )
                || signals.is_some_and(|signals| signals.cancel.is_cancelled());
            let changed = fresh.pending_amendment.is_some()
                || !self.contract.matches(fresh)
                || fresh.status != task.status
                || serde_json::to_value(&fresh.question).ok()
                    != serde_json::to_value(&task.question).ok()
                || signals.is_some_and(|signals| signals.pending_amend.lock().unwrap().is_some());
            publication = if stopped {
                Publication::Stopped
            } else if changed {
                Publication::Changed
            } else {
                Publication::Current
            };
            Ok(())
        })?;
        self.receipt.accept(task, &saved)?;
        Ok(publication)
    }
}

#[cfg(test)]
mod tests;
