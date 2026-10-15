use crate::task::state::{Task, TaskStatus};

use super::attempts::{self, AttemptEnd};

/// Rechecks the existing attempt after a permitted change to its checks or tree.
pub(crate) fn reopen(task: &mut Task, index: usize, now: i64) {
    let task = &mut task.data;
    let attempt = &mut task.attempts[index];
    attempt.status = crate::task::AttemptStatus::Running;
    attempt.ended_at = None;
    attempt.failure = None;
    task.status = TaskStatus::Running;
    task.updated_at = now;
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum Completion {
    Done,
    WaitingForLanding,
}

/// Parents have no implement attempt to end; ordinary tasks pass its index.
pub(crate) fn complete(task: &mut Task, attempt: Option<usize>, outcome: Completion, now: i64) {
    if let Some(index) = attempt {
        attempts::end(task, index, AttemptEnd::Passed, now);
    }
    let task = &mut task.data;
    task.status = match outcome {
        Completion::Done => TaskStatus::Done,
        Completion::WaitingForLanding => TaskStatus::Landing,
    };
    task.updated_at = now;
}

/// Counts each transition the existing failure notice observes.
pub(crate) fn fail(task: &mut Task, now: i64) {
    let task = &mut task.data;
    task.status = TaskStatus::Failed;
    task.failed_count += 1;
    task.updated_at = now;
}

pub(crate) fn pause_for_limit(task: &mut Task, index: usize, until: i64, now: i64) {
    attempts::end(task, index, AttemptEnd::Interrupted, now);
    let task = &mut task.data;
    task.queue.rate_pauses += 1;
    task.queue.paused_attempts += 1;
    task.queue.paused_until = Some(until);
    task.status = TaskStatus::Queued;
    task.updated_at = now;
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum PauseEnd {
    Elapsed,
    Cancelled { shutting_down: bool },
}

pub(crate) fn end_limit_pause(task: &mut Task, outcome: PauseEnd, now: i64) {
    let task = &mut task.data;
    if !matches!(
        outcome,
        PauseEnd::Cancelled {
            shutting_down: true
        }
    ) {
        task.queue.paused_until = None;
    }
    task.updated_at = now;
}

/// Applied after recovery interrupted a run. A saved stop or park keeps its meaning.
pub(crate) fn recover_interruption(task: &mut Task, now: i64) {
    let task = &mut task.data;
    if task.queue.stopped_explicitly {
        task.status = TaskStatus::Stopped;
        task.question = None;
    } else if task.queue.park.is_some() {
        task.status = TaskStatus::Queued;
        task.question = None;
    } else if !matches!(
        task.status,
        TaskStatus::Stopped | TaskStatus::Failed | TaskStatus::Done
    ) {
        task.status = TaskStatus::Queued;
    }
    task.updated_at = now;
}

impl Task {
    /// Puts the task into `failed` and counts the transition.
    pub fn mark_failed(&mut self) {
        fail(self, self.updated_at);
    }
}

/// A new base changes the context in which grounding and feasibility were observed.
pub(crate) fn move_base(task: &mut Task, base: String) {
    let task = &mut task.data;
    if task.base_sha != base {
        task.base_sha = base;
        super::checks::baseline::invalidate(task);
    }
}
