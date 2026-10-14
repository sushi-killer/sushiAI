//! A task the rules cannot move on is never parked silently. It retries after
//! a pause, twice at most, and then ends `Failed` with the reason in its
//! decisions and one note to the orchestrator.

use super::*;

/// How long a stuck task pauses before its next try, in seconds; a test run
/// may shorten it with `ORCHD_STUCK_PAUSE_SECS`.
const RETRY_PAUSE_SECS: i64 = 30 * 60;
/// How many pauses a task gets before it fails.
const MAX_RETRIES: usize = 2;
const RETRY_PREFIX: &str = "Retry later: ";

fn pause_secs() -> i64 {
    std::env::var("ORCHD_STUCK_PAUSE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(RETRY_PAUSE_SECS)
}

fn retries_so_far(task: &Task) -> usize {
    task.decisions
        .iter()
        .filter(|d| d.starts_with(RETRY_PREFIX))
        .count()
}

/// Replaces an eternal park. `true`: the task goes on after a pause (the loop
/// sleeps through `paused_until` holding no slot). `false`: it has failed and
/// the caller must stop asking about it.
pub(super) fn retry_or_fail(task: &mut Task, reason: &str) -> bool {
    task.question = None;
    let tried = retries_so_far(task);
    if tried < MAX_RETRIES {
        task.decisions.push(format!(
            "{RETRY_PREFIX}{reason}; trying again in {} s (retry {} of {MAX_RETRIES})",
            pause_secs(),
            tried + 1
        ));
        task.queue.paused_until = Some(now_ms() + pause_secs() * 1000);
        task.status = TaskStatus::Queued;
        return true;
    }
    task.decisions.push(format!(
        "Failed: {reason}; {MAX_RETRIES} later retries did not help"
    ));
    task.status = TaskStatus::Failed;
    false
}

/// Tells the orchestrator once that `task` failed here (when auto-answer is
/// on): the owner hears of it from there, not from a silent board.
pub(super) fn wake_orchestrator_for_stuck(app: &App, task: &mut Task) {
    if task.status != TaskStatus::Failed
        || task.parent.is_some()
        || task.archived
        || task.overseer_woken
        || !app.settings.read().unwrap().auto_answer
    {
        return;
    }
    let why = task.decisions.last().cloned().unwrap_or_default();
    let prompt = format!(
        "Task {} ({}) failed and no rule can move it on: {why}. Tell the owner in one short note what failed and what to do.",
        task.id, task.title
    );
    task.overseer_woken = true;
    app.save_task_logged(task);
    chat::wake_overseer(app, &task.repo, prompt, false, "failed", Some(&task.id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::task_with_status;

    #[test]
    fn a_stuck_task_pauses_twice_then_fails_with_its_reason() {
        let mut task = task_with_status(TaskStatus::Running);
        assert!(retry_or_fail(&mut task, "x broke"));
        assert!(task.queue.paused_until.is_some());
        assert_eq!(task.status, TaskStatus::Queued);
        assert!(retry_or_fail(&mut task, "x broke"));
        assert!(!retry_or_fail(&mut task, "x broke"));
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task
            .decisions
            .last()
            .unwrap()
            .starts_with("Failed: x broke"));
    }
}
