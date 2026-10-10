use super::*;

/// Backoff for consecutive usage or rate limit pauses, in minutes; the last
/// step repeats.
const BACKOFF_MINUTES: [u64; 3] = [5, 15, 45];
/// The longest one pause waits, whatever the message says.
const MAX_PAUSE_SECS: u64 = 6 * 3600;
/// Pauses in a row before the limit counts as an ordinary failure.
const MAX_PAUSES: u32 = 12;

/// Whether a harness error line reports a usage or rate limit. Substrings
/// only: no harness message shape is pinned in this repo.
pub(super) fn is_rate_limit(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    [
        "rate limit",
        "rate_limit",
        "usage limit",
        "too many requests",
    ]
    .iter()
    .any(|needle| e.contains(needle))
        // A bare 429 is a line number or a port as often as a status.
        || [
            "429 too many",
            "status 429",
            "status code 429",
            "\"status\":429",
            "\"status\": 429",
            "http 429",
            "http/1.1 429",
            "http/2 429",
        ]
        .iter()
        .any(|needle| e.contains(needle))
}

/// The reset time (Unix seconds) a message states after a `|`, the shape the
/// Claude CLI has printed ("... limit reached|1760000000").
fn stated_reset(error: &str) -> Option<i64> {
    let digits: String = error
        .rsplit_once('|')?
        .1
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() == 10).then(|| digits.parse().ok()).flatten()
}

/// How long to pause for `error`, or `None` when it is no limit (or the
/// pauses ran out): the stated reset time, else 5, 15, then 45 minutes;
/// at most six hours.
pub(super) fn rate_limit_wait_secs(error: &str, pauses: u32, now_secs: i64) -> Option<u64> {
    if !is_rate_limit(error) || pauses >= MAX_PAUSES {
        return None;
    }
    let backoff = BACKOFF_MINUTES[(pauses as usize).min(BACKOFF_MINUTES.len() - 1)] * 60;
    let wait = match stated_reset(error) {
        Some(reset) if reset > now_secs => (reset - now_secs) as u64 + 30,
        _ => backoff,
    };
    Some(wait.min(MAX_PAUSE_SECS))
}

/// Implement attempts that count toward `maxAttempts`: not an interrupted
/// one, nor a harness error that changed no file.
pub(super) fn counted_attempts(task: &Task) -> u32 {
    task.attempts
        .iter()
        .filter(|a| a.stage == Stage::Implement)
        .filter(|a| a.status != AttemptStatus::Interrupted)
        .filter(|a| {
            !(a.changed_files.is_empty()
                && a.failure.as_ref().map(|f| f.kind) == Some(FailureKind::Error))
        })
        .count() as u32
}

/// Pauses a task whose attempt hit a usage or rate limit: the attempt is
/// interrupted (not a failure), the task shows when it resumes and holds no
/// slot meanwhile. `false` when the task was stopped during the pause.
#[allow(clippy::too_many_arguments)]
pub(super) async fn pause_for_rate_limit(
    app: &Arc<App>,
    task_id: &str,
    task: &mut Task,
    idx: usize,
    error: &str,
    wait_secs: u64,
    cancel: &CancelToken,
    permit: &mut Option<tokio::sync::OwnedSemaphorePermit>,
) -> bool {
    let attempt_n = task.attempts[idx].n;
    task.attempts[idx].status = AttemptStatus::Interrupted;
    task.attempts[idx].ended_at = Some(now_ms());
    task.queue.rate_pauses += 1;
    task.queue.paused_attempts += 1;
    task.queue.paused_until = Some(now_ms() + wait_secs as i64 * 1000);
    let line = format!(
        "Paused: usage or rate limit ({}); resuming in {} min, this is not a failed attempt",
        truncate_chars(error.trim(), 120),
        wait_secs.div_ceil(60)
    );
    app.broadcast_log(task_id, attempt_n, line.clone());
    task.decisions.push(line);
    task.status = TaskStatus::Queued;
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    permit.take();
    let resumed = tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(Duration::from_secs(wait_secs)) => true,
    };
    // A shutdown keeps the mark: the next daemon serves the rest of the pause.
    if resumed || !app.shutting_down.load(Ordering::SeqCst) {
        task.queue.paused_until = None;
    }
    if !resumed {
        task.status = app.cancelled_status(&task.status);
    }
    task.updated_at = now_ms();
    let _ = app.store.save_task(task);
    app.broadcast_task(task);
    resumed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_messages_are_told_from_other_errors() {
        for e in [
            "Claude AI usage limit reached|1760000000",
            "429 Too Many Requests",
            "request failed: HTTP 429",
            "{\"status\":429,\"error\":\"x\"}",
            "Rate limit exceeded, retry later",
            "error: rate_limit_error",
        ] {
            assert!(is_rate_limit(e), "{e}");
        }
        for e in [
            "model not supported",
            "Claude exited with an error.",
            "port 14290 is busy",
            "TypeError at cli.js:429:15",
        ] {
            assert!(!is_rate_limit(e), "{e}");
        }
    }

    #[test]
    fn the_wait_backs_off_then_follows_a_stated_reset_and_is_capped() {
        let now = 1_760_000_000;
        let secs = |e: &str, n| rate_limit_wait_secs(e, n, now);
        assert_eq!(secs("rate limit", 0), Some(300));
        assert_eq!(secs("rate limit", 1), Some(900));
        assert_eq!(secs("rate limit", 2), Some(2700));
        assert_eq!(secs("rate limit", 7), Some(2700));
        assert_eq!(secs("usage limit reached|1760003600", 3), Some(3630));
        assert_eq!(
            secs("usage limit reached|1760999999", 0),
            Some(MAX_PAUSE_SECS)
        );
        // A reset in the past falls back to the backoff.
        assert_eq!(secs("usage limit reached|1759999000", 0), Some(300));
        assert_eq!(secs("rate limit", MAX_PAUSES), None);
        assert_eq!(secs("model not supported", 0), None);
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
        task.attempts = vec![interrupted, error, edited_error, verify];
        assert_eq!(implement_attempt_count(&task), 4);
        assert_eq!(counted_attempts(&task), 2);
        assert_eq!(completed_implement_attempts(&task), 3);
    }

    #[tokio::test]
    async fn a_pause_shows_when_it_ends_holds_no_slot_and_is_no_failed_attempt() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.attempts = vec![attempt_with_failure(1, "x")];
        task.attempts[0].status = AttemptStatus::Running;
        task.attempts[0].failure = None;
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let cancel = CancelToken::new();
        let mut permit = Some(app.slots.clone().acquire_owned().await.unwrap());
        let before = app.slots.available_permits();
        let paused = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                let ok = pause_for_rate_limit(
                    &app,
                    &id,
                    &mut task,
                    0,
                    "rate limit hit",
                    1,
                    &cancel,
                    &mut permit,
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
        app.store.list_tasks().unwrap().remove(0).id
    }

    #[tokio::test]
    async fn a_pause_cut_short_by_a_shutdown_keeps_its_mark() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.attempts = vec![attempt_with_failure(1, "x")];
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let cancel = CancelToken::new();
        let waiter = {
            let (app, cancel) = (app.clone(), cancel.clone());
            tokio::spawn(async move {
                let ok = pause_for_rate_limit(
                    &app,
                    &id,
                    &mut task,
                    0,
                    "rate limit",
                    600,
                    &cancel,
                    &mut None,
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
