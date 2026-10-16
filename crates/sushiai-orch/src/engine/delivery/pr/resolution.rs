use crate::engine::questions::wake::wake_orchestrator_for_waiting;
use crate::engine::run_control::TaskControl;
use crate::engine::state::App;
use crate::task::{Task, TaskStatus};

#[derive(PartialEq)]
enum Intent {
    Repair { cause: String, step: u32 },
    Conflict { tip: Option<String> },
}

/// The persisted resolution request whose async preparation is being handed back.
#[derive(PartialEq)]
pub(super) struct ResolutionReceipt {
    id: String,
    intent: Intent,
}

pub(super) enum Handoff {
    Active(Box<Task>),
    Deferred,
}

impl ResolutionReceipt {
    pub(super) fn capture(task: &Task) -> Result<Self, String> {
        let intent = if let Some(repair) = task.queue.pr_check_repair.as_ref() {
            Intent::Repair {
                cause: repair.cause.clone(),
                step: repair.step,
            }
        } else if task.queue.pr_conflict_tried {
            Intent::Conflict {
                tip: task.fresh_base.as_ref().and_then(|base| base.tip.clone()),
            }
        } else {
            return Err("the task has no pending PR resolution".to_string());
        };
        Ok(Self {
            id: task.id.clone(),
            intent,
        })
    }

    /// Admission and existing-control creation share the Owner mutation lock.
    pub(super) fn resume(&self, app: &App) -> Result<Handoff, String> {
        let lock = app.task_lock(&self.id);
        let guard = lock.lock().unwrap();
        let fresh = app
            .store
            .load_task(&self.id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "task not found".to_string())?;
        if fresh.archived
            || fresh.queue.park.is_some()
            || fresh.queue.stopped_explicitly
            || Self::capture(&fresh).ok().as_ref() != Some(self)
        {
            return Ok(Handoff::Deferred);
        }
        let mut controls = app.controls.lock().unwrap();
        if controls
            .get(&self.id)
            .is_some_and(|control| control.signals.cancel.is_cancelled())
        {
            return Ok(Handoff::Deferred);
        }
        match fresh.status {
            TaskStatus::Queued => {
                controls
                    .entry(self.id.clone())
                    .or_insert_with(|| TaskControl::spawn(app.arc(), self.id.clone(), true));
            }
            TaskStatus::Running if controls.contains_key(&self.id) => {}
            TaskStatus::Waiting if fresh.question.is_some() => {}
            _ => return Ok(Handoff::Deferred),
        }
        drop(controls);
        drop(guard);
        if fresh.status == TaskStatus::Waiting {
            wake_orchestrator_for_waiting(app, &fresh);
        }
        Ok(Handoff::Active(Box::new(fresh)))
    }
}
