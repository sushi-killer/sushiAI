use crate::task::{Backlog, ParkRecord, Task, TaskStatus, Wait};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Starter {
    Owner,
    Autopilot,
}

impl Task {
    pub(crate) fn clear_park(&mut self) {
        self.data.queue.park = None;
    }
    pub(crate) fn set_backlog(&mut self, backlog: Option<Backlog>) {
        self.data.queue.backlog = backlog;
    }
    pub(crate) fn set_wait(&mut self, wait: Option<Wait>) {
        self.data.queue.wait = wait;
    }
    pub(crate) fn reset_rate_pauses(&mut self) {
        self.data.queue.rate_pauses = 0;
    }
    pub(crate) fn remind_waiting(&mut self, now: i64) {
        self.data.queue.wait_reminded_at = Some(now);
    }
    pub(crate) fn start_queued(&mut self, by: Starter, restart_children: bool, now: i64) {
        let task = &mut self.data;
        if restart_children {
            task.queue.restart_children = true;
            task.question = None;
        }
        if matches!(task.status, TaskStatus::Stopped | TaskStatus::Failed)
            || task.queue.restart_children
        {
            task.status = TaskStatus::Queued;
        }
        let resumed = task.queue.park.take().is_some();
        task.queue.stopped_explicitly = false;
        task.loop_state.stuck_retries = 0;
        if resumed {
            task.decisions.push("Owner: resumed a parked task".into());
        }
        if task.queue.backlog.take().is_some() {
            task.decisions.push(
                match by {
                    Starter::Owner => "Owner: started from the backlog",
                    Starter::Autopilot => "Autopilot: started from the next backlog",
                }
                .into(),
            );
        }
        task.updated_at = now;
    }
    pub(crate) fn interrupt(
        &mut self,
        park: Option<ParkRecord>,
        explicit: bool,
        line: String,
        now: i64,
    ) {
        let task = &mut self.data;
        task.question = None;
        task.queue.park = park;
        task.queue.stopped_explicitly = explicit;
        task.status = if task.queue.park.is_some() {
            TaskStatus::Queued
        } else {
            TaskStatus::Stopped
        };
        if !task.decisions.contains(&line) {
            task.decisions.push(line);
        }
        if let Some(p) = &task.queue.park {
            let note = format!("Park: {} (by {})", p.reason, p.by.label());
            if !task.decisions.contains(&note) {
                task.decisions.push(note);
            }
        }
        task.updated_at = now;
    }
    pub(crate) fn finish_cancellation(&mut self, status: TaskStatus) {
        let task = &mut self.data;
        task.status = if task.queue.park.is_some() {
            TaskStatus::Queued
        } else {
            status
        };
        if task.status == TaskStatus::Stopped || task.queue.park.is_some() {
            task.question = None;
        }
    }
    pub(crate) fn defer_stuck_retry(&mut self, limit: u32, until: i64) -> Option<u32> {
        self.data.question = None;
        if self.loop_state.stuck_retries >= limit {
            return None;
        }
        self.data.loop_state.stuck_retries += 1;
        self.data.queue.paused_until = Some(until);
        self.data.status = TaskStatus::Queued;
        Some(self.loop_state.stuck_retries)
    }
}
