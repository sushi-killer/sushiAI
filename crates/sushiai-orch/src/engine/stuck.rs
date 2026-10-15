//! A task the rules cannot move on is never parked silently. It retries after
//! a pause, twice at most, and then ends `Failed` with the reason in its
//! decisions and one note to the orchestrator.

#[cfg(test)]
use crate::task::TaskStatus;

use crate::clock::now_ms;
use crate::task::Task;

/// How long a stuck task pauses before its next try, in seconds; a test run
/// may shorten it with `ORCHD_STUCK_PAUSE_SECS`.
const RETRY_PAUSE_SECS: i64 = 30 * 60;
/// How many pauses a task gets before it fails.
const MAX_RETRIES: u32 = 2;
const RETRY_PREFIX: &str = "Retry later: ";

fn pause_secs() -> i64 {
    std::env::var("ORCHD_STUCK_PAUSE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(RETRY_PAUSE_SECS)
}

/// Replaces an eternal park. `true`: the task goes on after a pause (the loop
/// sleeps through `paused_until` holding no slot). `false`: it has failed and
/// the caller must stop asking about it.
pub(super) fn retry_or_fail(task: &mut Task, reason: &str) -> bool {
    task.clear_question();
    if let Some(tried) = task.defer_stuck_retry(MAX_RETRIES, now_ms() + pause_secs() * 1000) {
        task.record_decision(format!(
            "{RETRY_PREFIX}{reason}; trying again in {} s (retry {} of {MAX_RETRIES})",
            pause_secs(),
            tried
        ));
        return true;
    }
    task.record_decision(format!(
        "Failed: {reason}; {MAX_RETRIES} later retries did not help"
    ));
    task.mark_failed();
    false
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
        assert_eq!(task.loop_state.stuck_retries, MAX_RETRIES);
    }
}
