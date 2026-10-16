#[cfg(test)]
use crate::task::AttemptStatus;
#[cfg(test)]
use crate::task::FailureKind;

use crate::clip::truncate_chars;
use crate::clock::now_ms;
#[cfg(test)]
use crate::engine::decision::completed_implement_attempts;
#[cfg(test)]
use crate::engine::decision::implement_attempt_count;
use crate::engine::questions::parking::QuestionSlot;
#[cfg(test)]
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::attempt_with_failure;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::harness;
#[cfg(test)]
use crate::harness::cancel::CancelToken;
use crate::task::Task;
use crate::task::TaskStatus;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// Backoff for consecutive usage or rate limit pauses, in minutes; the last
/// step repeats.
const BACKOFF_MINUTES: [u64; 3] = [5, 15, 45];
/// The longest one pause waits, whatever the message says.
const MAX_PAUSE_SECS: u64 = 6 * 3600;
/// Pauses in a row before the limit counts as an ordinary failure.
const MAX_PAUSES: u32 = 12;

/// How long to pause for a provider failure, or `None` when it is no limit
/// (or the pauses ran out): the stated reset time, else 5, 15, then 45
/// minutes; at most six hours.
pub(super) fn rate_limit_wait_secs(failure: harness::ProviderFailure, pauses: u32) -> Option<u64> {
    let retry_after_ms = match failure {
        harness::ProviderFailure::RateLimited { retry_after_ms }
        | harness::ProviderFailure::Overloaded { retry_after_ms } => retry_after_ms,
        harness::ProviderFailure::QuotaExhausted { .. }
        | harness::ProviderFailure::Auth
        | harness::ProviderFailure::Other => return None,
    };
    if pauses >= MAX_PAUSES {
        return None;
    }
    let backoff = BACKOFF_MINUTES[(pauses as usize).min(BACKOFF_MINUTES.len() - 1)] * 60;
    let wait = retry_after_ms.map_or(backoff, |ms| ms / 1000);
    Some(wait.min(MAX_PAUSE_SECS))
}

/// Implement attempts that count toward `maxAttempts`: not an interrupted
/// one, nor a harness error that changed no file.
pub(super) fn counted_attempts(task: &Task) -> u32 {
    crate::task::attempts::counted_implement_count(task)
}

/// Pauses a task whose attempt hit a usage or rate limit: the attempt is
/// interrupted (not a failure), the task shows when it resumes and holds no
/// slot meanwhile. `false` when the task was stopped during the pause.
pub(super) async fn pause_for_rate_limit(
    app: &Arc<App>,
    task: &mut Task,
    error: &str,
    wait_secs: u64,
    slot: &mut QuestionSlot<'_>,
) -> bool {
    let idx = task.attempts.len() - 1;
    let attempt_n = task.attempts[idx].n;
    let now = now_ms();
    crate::task::transitions::pause_for_limit(task, idx, now + wait_secs as i64 * 1000, now);
    let line = format!(
        "Paused: usage or rate limit ({}); resuming in {} min, this is not a failed attempt",
        truncate_chars(error.trim(), 120),
        wait_secs.div_ceil(60)
    );
    app.broadcast_log(&task.id, attempt_n, line.clone());
    task.record_decision(line);
    task.resume_phase(TaskStatus::Queued, now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    let resumed = slot.pause(Duration::from_secs(wait_secs)).await;
    let outcome = if resumed {
        crate::task::transitions::PauseEnd::Elapsed
    } else {
        crate::task::transitions::PauseEnd::Cancelled {
            shutting_down: app.shutting_down.load(Ordering::SeqCst),
        }
    };
    crate::task::transitions::end_limit_pause(task, outcome, now_ms());
    if !resumed {
        app.apply_cancellation(task);
    }
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
    resumed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_backs_off_then_follows_a_stated_reset_and_is_capped() {
        let limit = |retry_after_ms| harness::ProviderFailure::RateLimited { retry_after_ms };
        let secs = rate_limit_wait_secs;
        assert_eq!(secs(limit(None), 0), Some(300));
        assert_eq!(secs(limit(None), 1), Some(900));
        assert_eq!(secs(limit(None), 2), Some(2700));
        assert_eq!(secs(limit(None), 7), Some(2700));
        assert_eq!(secs(limit(Some(3_630_000)), 3), Some(3630));
        assert_eq!(
            secs(limit(Some(MAX_PAUSE_SECS * 1000 + 1000)), 0),
            Some(MAX_PAUSE_SECS)
        );
        let overloaded = harness::ProviderFailure::Overloaded {
            retry_after_ms: None,
        };
        assert_eq!(secs(overloaded, 0), Some(300));
        assert_eq!(secs(limit(None), MAX_PAUSES), None);
        assert_eq!(secs(harness::ProviderFailure::Auth, 0), None);
        assert_eq!(secs(harness::ProviderFailure::Other, 0), None);
        let billing = harness::ProviderFailure::QuotaExhausted {
            retry_after_ms: None,
        };
        assert_eq!(secs(billing, 0), None);
    }

    #[test]
    fn interrupted_attempts_and_changeless_errors_do_not_count() {
        let mut task = task_with_status(TaskStatus::Running);
        let mut interrupted = attempt_with_failure(1, "x");
        interrupted.status = AttemptStatus::Interrupted;
        let mut error = attempt_with_failure(2, "error:x");
        error.failure.as_mut().unwrap().kind = FailureKind::Error;
        let mut edited_error = attempt_with_failure(3, "error:y");
        edited_error.failure.as_mut().unwrap().kind = FailureKind::Error;
        edited_error.changed_files = vec!["a.rs".into()];
        let verify = attempt_with_failure(4, "verify:z");
        task.fixture_data().attempts = vec![interrupted, error, edited_error, verify];
        assert_eq!(implement_attempt_count(&task), 4);
        assert_eq!(counted_attempts(&task), 2);
        assert_eq!(completed_implement_attempts(&task), 3);
    }

    #[tokio::test]
    async fn a_pause_shows_when_it_ends_holds_no_slot_and_is_no_failed_attempt() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = vec![attempt_with_failure(1, "x")];
        task.fixture_data().attempts[0].status = AttemptStatus::Running;
        task.fixture_data().attempts[0].failure = None;
        app.store.save_task(&task).unwrap();
        let cancel = CancelToken::new();
        let mut permit = Some(app.slots.clone().acquire_owned().await.unwrap());
        let before = app.slots.available_permits();
        let paused = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                let ok = pause_for_rate_limit(
                    &app,
                    &mut task,
                    "rate limit hit",
                    1,
                    &mut QuestionSlot::new(
                        &TaskSignals {
                            cancel: cancel.clone(),
                            ..TaskSignals::new()
                        },
                        &mut permit,
                    ),
                )
                .await;
                (ok, task)
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(app.slots.available_permits(), before + 1);
        let mid = app.store.load_task(&paused_id(&app)).unwrap().unwrap();
        assert!(mid.queue.paused_until.is_some());
        assert_eq!(mid.status, TaskStatus::Queued);
        let (resumed, task) = paused.await.unwrap();
        assert!(resumed);
        assert_eq!(task.queue.paused_until, None);
        assert_eq!(task.queue.rate_pauses, 1);
        assert_eq!(task.attempts[0].status, AttemptStatus::Interrupted);
        assert!(task.attempts[0].failure.is_none());
        assert!(task.decisions[0].starts_with("Paused: usage or rate limit"));
        assert_eq!(counted_attempts(&task), 0);
    }

    fn paused_id(app: &App) -> String {
        app.store.list_tasks().unwrap().remove(0).id.clone()
    }

    #[tokio::test]
    async fn a_pause_cut_short_by_a_shutdown_keeps_its_mark() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = vec![attempt_with_failure(1, "x")];
        app.store.save_task(&task).unwrap();
        let cancel = CancelToken::new();
        let waiter = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                let ok = pause_for_rate_limit(
                    &app,
                    &mut task,
                    "rate limit",
                    600,
                    &mut QuestionSlot::new(
                        &TaskSignals {
                            cancel: cancel.clone(),
                            ..TaskSignals::new()
                        },
                        &mut None,
                    ),
                )
                .await;
                (ok, task)
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        app.shutting_down.store(true, Ordering::SeqCst);
        cancel.cancel();
        let (resumed, task) = waiter.await.unwrap();
        assert!(!resumed);
        assert!(task.queue.paused_until.is_some());
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert!(saved.queue.paused_until.is_some());
        assert_eq!(saved.status, TaskStatus::Queued);
    }
}
