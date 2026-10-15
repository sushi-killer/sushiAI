//! The failure ladder: what happens when attempts keep failing, so a task ends
//! in `failed` with a report instead of an open question. Environment failures
//! (a timeout, a flaky or outside-the-diff test, an install error) say nothing
//! about the approach and never count as "the same failure again".
//!
//! Rungs, in order: the orchestrator's continues (`MAX_ORCHESTRATOR_CONTINUES`,
//! only with auto-answer on), one changed approach (a higher tier when the
//! settings define one, and a forced advisor run on the planner route that
//! re-plans the remaining work), then `failed`.

#[cfg(test)]
use crate::engine::decision::fail_and_continue;
#[cfg(test)]
use crate::engine::lifecycle::attempt_files::AttemptFiles;
#[cfg(test)]
use crate::engine::questions::parking::QuestionSlot;
#[cfg(test)]
use crate::engine::run_control::TaskSignals;
#[cfg(test)]
use crate::policy::variant::Variant;
#[cfg(test)]
use crate::task::AnsweredBy;
#[cfg(test)]
use crate::task::AskedBy;
#[cfg(test)]
use crate::task::Attempt;
#[cfg(test)]
use crate::task::Question;

#[cfg(test)]
use crate::engine::decision::advance_after_failure;
#[cfg(test)]
use crate::engine::decision::advance_after_failure_with;
#[cfg(test)]
use crate::engine::decision::LoopSignal;
#[cfg(test)]
use crate::engine::graph::dependencies::DEPENDENCY_QUESTION;
#[cfg(test)]
use crate::policy::routing::Tier;
#[cfg(test)]
use crate::task::VerifyOutcome;

use crate::clock::now_ms;
use crate::engine::decision;
use crate::engine::decision::implement_attempt_count;
use crate::engine::decision::EXHAUSTED_ANSWER_PREFIX;
use crate::engine::state::App;
#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
use crate::harness::cancel::CancelToken;
use crate::policy::routing::Route;
use crate::policy::settings::Settings;
use crate::prompts::clip::truncate_chars;
use crate::task::AttemptStatus;
use crate::task::Blame;
use crate::task::FailureKind;
use crate::task::QuestionKind;
use crate::task::Stage;
use crate::task::Task;
use crate::task::TaskStatus;
use crate::task::UnreviewedPart;
use serde_json::json;
use std::time::Duration;

use crate::task::notes::overseer::Entry;

/// Decision line a changed approach leaves for the owner to read.
pub(super) fn approach_line(attempt_n: u32) -> String {
    format!("Ladder: changed approach after attempt {attempt_n}")
}

/// Whether the ladder already changed the approach once for this task.
pub(super) fn approach_changed(task: &Task) -> bool {
    task.loop_state.approach_changed_after.is_some()
}

/// Whether the failed attempt the next one follows is the one the ladder
/// changed the approach after: that attempt gets the advisor, forced.
pub(super) fn approach_pending(task: &Task) -> bool {
    task.attempts
        .iter()
        .rfind(|a| a.stage == Stage::Implement)
        .is_some_and(|a| task.loop_state.approach_changed_after == Some(a.n))
}

/// What the base checkout printed for a check command (`None`: no run known).
pub(super) type BaseTail<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The base results `app` holds for `base_sha`, by command.
pub(super) fn base_tails<'a>(app: &'a App, base_sha: &str) -> impl Fn(&str) -> Option<String> + 'a {
    let base_sha = base_sha.to_string();
    move |command| {
        app.base_runs
            .lock()
            .unwrap()
            .get(&(base_sha.clone(), command.to_string()))
            .map(|run| run.tail.clone())
    }
}

/// Whether attempt `at` failed on its checks for a reason that is the
/// environment, not the approach. A timeout or an install error always is.
/// A flaky or outside-the-diff signal only is when the evidence backs it: every
/// failing test also fails on the base, or a test passed and failed on the
/// same tree. A change that breaks a test it did not touch is not.
pub(super) fn environment_failure(task: &Task, at: usize, base: BaseTail) -> bool {
    let signals = crate::task::attempts::signals::attempt_signals(task, at);
    if signals.iter().any(|s| s == "timeout" || s == "install") {
        return true;
    }
    if signals.is_empty() {
        return false;
    }
    let attempt = &task.attempts[at];
    attempt
        .verify
        .iter()
        .filter(|o| o.code != Some(0))
        .any(|o| {
            let now = crate::policy::test_output::failing_tests(&o.tail);
            if now.is_empty() {
                return false;
            }
            let on_base = base(&o.command).map(|t| crate::policy::test_output::failing_tests(&t));
            if on_base.is_some_and(|b| now.iter().all(|t| b.contains(t))) {
                return true;
            }
            // The same tree ran this command before with another outcome.
            task.attempts[..at].iter().any(|earlier| {
                earlier.changed_files == attempt.changed_files
                    && earlier.diff_stat == attempt.diff_stat
                    && earlier
                        .verify
                        .iter()
                        .filter(|e| e.command == o.command)
                        .any(|e| crate::policy::test_output::failing_tests(&e.tail) != now)
            })
        })
}

/// Whether a task failed on the environment: a timeout, or a check failure
/// that [`environment_failure`] holds for its last implement attempt.
pub(super) fn failed_on_environment(task: &Task, base: BaseTail) -> bool {
    let Some(at) = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)
    else {
        return task
            .attempts
            .last()
            .and_then(|a| a.failure.as_ref())
            .is_some_and(|f| f.detail.contains("timed out"));
    };
    environment_failure(task, at, base)
        || task.attempts[at].failure.as_ref().is_some_and(|f| {
            matches!(f.kind, FailureKind::Stall | FailureKind::TimeBudget)
                || f.detail.contains("timed out")
        })
}

/// The one rung that changes how the next attempt works: the tier goes up
/// when the settings name a different route for the higher tier, and the
/// next attempt starts with a forced advisor run (the planner's route
/// re-reads the failure and names a different plan for the rest of the work).
pub(super) fn change_approach(task: &mut Task, settings: &Settings, attempt_n: u32) {
    let up = task.tier.up();
    let promote = up != task.tier && settings.tiers.get(&up) != settings.tiers.get(&task.tier);
    let tier_note = if promote {
        format!("tier {}", up.as_str())
    } else {
        "the same tier".to_string()
    };
    let tier = promote.then_some(up);
    task.change_approach(
        tier,
        attempt_n,
        format!(
            "{} ({tier_note}; the planner re-plans the rest)",
            approach_line(attempt_n)
        ),
    );
}

/// The text the owner reads when a task ends `failed`.
pub(super) fn failure_report(task: &Task) -> String {
    let last = task.attempts.iter().rfind(|a| a.failure.is_some());
    let cause = last
        .and_then(|a| a.failure.as_ref())
        .map(|f| {
            let first = f
                .detail
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            format!(
                "{}: {}",
                decision::signature_phrase(&f.signature),
                truncate_chars(first.trim(), 160)
            )
        })
        .unwrap_or_else(|| "an error with no detail".to_string());
    let tried: Vec<&str> = task
        .decisions
        .iter()
        .filter(|d| d.starts_with("Ladder:") || d.starts_with(EXHAUSTED_ANSWER_PREFIX))
        .map(String::as_str)
        .collect();
    let mut text = format!(
        "Task \"{}\" failed after {} attempt(s). Last failure: {cause}.",
        task.title,
        implement_attempt_count(task)
    );
    if !tried.is_empty() {
        text.push_str(&format!(" Tried: {}.", tried.join("; ")));
    }
    if !task.branch.is_empty() {
        text.push_str(&format!(
            " The work stays on branch {}; nothing was merged or pushed.",
            task.branch
        ));
    }
    text
}

/// Ends the ladder: the task is `failed`, no question is left open. The
/// caller returns `Stop`; the loop's end tells the owner (`notify_task_failed`).
pub(super) fn end_ladder(app: &App, task: &mut Task) {
    let signature = task
        .attempts
        .iter()
        .rfind(|a| a.failure.is_some())
        .and_then(|a| a.failure.as_ref())
        .map(|f| decision::signature_phrase(&f.signature))
        .unwrap_or_else(|| "a failure".to_string());
    task.clear_question();
    task.mark_failed();
    task.record_decision(format!(
        "Ladder: stopped after {} attempt(s) on {signature}; continue, a changed approach and the orchestrator's continues were used up",
        implement_attempt_count(task)
    ));
    task.touch(now_ms());
    app.save_task_logged(task);
    app.broadcast_task(task);
}

/// A route other than `current` (another harness first, else another model)
/// that can run, for a review that gave no verdict. `None` when the settings
/// hold no other route.
pub(super) fn fallback_review_route(
    app: &App,
    settings: &Settings,
    current: &Route,
) -> Option<(Route, Option<String>)> {
    let same = |r: &Route| r.harness == current.harness && r.model == current.model;
    let mut others: Vec<&Route> = settings
        .routes
        .iter()
        .filter(|r| r.id != current.id && !same(r))
        .collect();
    others.sort_by_key(|r| r.harness == current.harness);
    others
        .into_iter()
        .find_map(|r| app.usable_route(settings, r.clone()))
        .filter(|(r, _)| r.id != current.id && !same(r))
}

/// A pause in milliseconds: the environment variable `name` when it holds a
/// number (the black-box tests shorten the pauses), else `default`, or
/// `in_tests` inside this crate's unit tests.
fn pause_ms(name: &str, default: u64, in_tests: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if cfg!(test) { in_tests } else { default })
}

/// The pause before the `n`th retry (1-based) of a setup step: it doubles each
/// time (`ORCHD_SETUP_RETRY_MS` sets the first).
pub(super) fn setup_retry_delay(n: u32) -> Duration {
    Duration::from_millis(pause_ms("ORCHD_SETUP_RETRY_MS", 5_000, 20) << n.saturating_sub(1).min(4))
}

/// How many times a failing worktree or setup step is tried again.
pub(super) const SETUP_RETRIES: u32 = 2;

/// Waits out `delay`; `false` when `cancel` fired first.
pub(super) async fn sleep_unless_cancelled(delay: Duration, cancel: &CancelToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(delay) => true,
    }
}

/// How long a dependency that failed on the environment waits before its one
/// automatic restart.
pub(super) fn dependency_restart_delay() -> Duration {
    Duration::from_millis(pause_ms("ORCHD_DEPENDENCY_RESTART_MS", 120_000, 50))
}

/// Decision line of the one restart of a failed task an agent may make.
pub(super) const AGENT_RESTART_LINE: &str = "Orchestrator: restarted a failed task";

/// An agent may restart a failed task once; the owner any time.
pub(super) fn agent_may_restart(task: &Task) -> bool {
    task.status != TaskStatus::Failed || !task.loop_state.agent_restarted
}

/// Whether a failed top-level task ended on the environment by the typed
/// cause of its last implement attempt, and no agent restarted it yet.
pub(super) fn restartable_on_environment(task: &Task) -> bool {
    task.status == TaskStatus::Failed
        && task.parent.is_none()
        && !task.archived
        && !task.queue.stopped_explicitly
        && agent_may_restart(task)
        && task
            .attempts
            .iter()
            .rev()
            .find(|a| a.stage == Stage::Implement)
            .is_some_and(|a| a.cause == Some(Blame::Environment))
}

/// A parent is unreviewed while any landed, unarchived subtask is: derived
/// each time it is read, never cleared, so the parent's own attempt cannot
/// land that work on the default branch.
pub(super) fn inherit_unreviewed(app: &App, task: &mut Task) {
    for child in app.repo_tasks(&task.repo) {
        if child.parent.as_deref() == Some(task.id.as_str())
            && child.unreviewed
            && child.status == TaskStatus::Done
            && !child.archived
        {
            task.inherit_unreviewed_part(
                UnreviewedPart {
                    id: child.id.clone(),
                    title: child.title.clone(),
                },
                crate::delivery::pr::body::unreviewed_part_line(&child),
            );
        }
    }
}

pub(super) const DEPENDENCY_RESTART_LINE: &str =
    "Orchestrator: restarted a dependency that failed on the environment";

/// The decision line of an attempt committed unreviewed because its open
/// "no verdict" question was retired.
pub(super) fn unreviewed_line(attempt_n: u32) -> String {
    format!("Orchestrator: attempt {attempt_n} committed unreviewed: its open review question was retired")
}

/// A task relaunched after its "no verdict" question was retired while no
/// loop was live: the blocked attempt, which only needs its final checks and
/// the commit.
pub(super) fn pending_unreviewed(task: &Task) -> Option<usize> {
    let idx = task
        .attempts
        .iter()
        .rposition(|a| a.stage == Stage::Implement)?;
    let a = &task.attempts[idx];
    (task.unreviewed
        && a.status == AttemptStatus::Blocked
        && task.loop_state.unreviewed_attempt == Some(a.n))
    .then_some(idx)
}

impl App {
    /// The shared once-only gate for an agent restart of the failed tasks
    /// `ids`: refuses when one was already restarted by an agent, else
    /// records the restart on each.
    pub(super) fn agent_restart_gate(&self, ids: &[String]) -> Result<(), String> {
        let failed: Vec<String> = ids
            .iter()
            .filter(|id| {
                self.store
                    .load_task(id)
                    .ok()
                    .flatten()
                    .is_some_and(|t| t.status == TaskStatus::Failed)
            })
            .cloned()
            .collect();
        for id in &failed {
            if let Ok(Some(t)) = self.store.load_task(id) {
                if !agent_may_restart(&t) {
                    return Err("this failed task was already restarted once by an agent: only the owner restarts it again".to_string());
                }
            }
        }
        for id in &failed {
            self.update_task(id, |t| {
                t.record_agent_restart(AGENT_RESTART_LINE.to_string());
            })?;
        }
        Ok(())
    }

    /// One-time migration of questions the failure ladder no longer asks.
    /// An open "attempts keep failing" question ends the ladder (the task
    /// fails with its report); an open "no verdict" question becomes the
    /// unreviewed path (the attempt is committed, marked, and not landed).
    pub(super) fn retire_ladder_questions(&self) {
        for mut task in self.store.list_tasks().unwrap_or_default() {
            let Some(kind) = task.question.as_ref().map(|q| q.kind) else {
                continue;
            };
            if task.status != TaskStatus::Waiting || task.archived {
                continue;
            }
            match kind {
                QuestionKind::AttemptsFailing => {
                    // The restart wake asks the overseer again; with nobody
                    // to ask, the ladder takes its next rung.
                    let asked_again = self.settings.read().unwrap().auto_answer
                        && (task.loop_state.non_owner_continues as usize)
                            < crate::engine::questions::exhausted::MAX_ORCHESTRATOR_CONTINUES;
                    if !asked_again {
                        self.step_unanswered_exhausted(&mut task);
                    }
                }
                QuestionKind::ReviewNoVerdict => {
                    let Some(n) = task
                        .attempts
                        .iter()
                        .rfind(|a| a.stage == Stage::Implement)
                        .map(|a| a.n)
                    else {
                        continue;
                    };
                    task.accept_unreviewed_attempt(n, unreviewed_line(n), now_ms());
                    self.save_task_logged(&task);
                    self.broadcast_task(&task);
                }
                _ => {}
            }
        }
    }

    /// A top-level task that finished with an attempt no review read tells
    /// the owner once that its work is unreviewed and was not landed.
    pub(super) fn notify_task_unreviewed(&self, task: &Task) {
        if !task.unreviewed || task.status != TaskStatus::Done || task.parent.is_some() {
            return;
        }
        let key = format!("{}|task_unreviewed|{}", task.id, task.attempts.len());
        if self
            .notice_log()
            .iter()
            .any(|e| e.decision_keys().any(|k| k == key))
        {
            return;
        }
        let text = format!(
            "Task \"{}\" is done but unreviewed: no review route gave a verdict. Its work was not landed; it waits on branch {} and a pull request marked unreviewed is opened.",
            task.title, task.branch
        );
        crate::engine::chat::store::post_note(self, &task.repo, &text);
        self.record_overseer(
            Entry::new(
                "asked_owner",
                &task.repo,
                Some(&task.id),
                &truncate_chars(&text, crate::task::notes::overseer::MAX_TEXT_CHARS),
                crate::clock::now_ms(),
            )
            .with_detail(&text)
            .with_decision(&[key]),
        );
    }

    /// A top-level task that ended `failed` tells the owner once, with the
    /// report, and wakes the overseer when auto-answer is on. The overseer log
    /// remembers the notice by a key made of the task and its attempt count,
    /// so a loop that ends again, a restart or a rebroadcast never repeats it.
    pub(super) fn notify_task_failed(&self, task: &Task) {
        if task.status != TaskStatus::Failed || task.parent.is_some() || task.archived {
            return;
        }
        // An environment failure is restarted once by code, like a dependency;
        // the owner hears of it only if the restart cannot start or fails again.
        if restartable_on_environment(task) && tokio::runtime::Handle::try_current().is_ok() {
            let (app, id) = (self.arc(), task.id.clone());
            tokio::spawn(async move {
                let started = app
                    .handle_task_start(json!({"id": id, "source": "orchd"}))
                    .await;
                if started.is_err() {
                    if let Ok(Some(failed)) = app.store.load_task(&id) {
                        app.announce_task_failed(&failed);
                    }
                }
            });
            return;
        }
        self.announce_task_failed(task);
    }

    fn announce_task_failed(&self, task: &Task) {
        let key = format!("{}|task_failed|{}", task.id, task.failed_count);
        if self
            .notice_log()
            .iter()
            .any(|e| e.decision_keys().any(|k| k == key))
        {
            return;
        }
        let report = failure_report(task);
        crate::engine::chat::store::post_note(self, &task.repo, &report);
        self.record_overseer(
            Entry::new(
                "asked_owner",
                &task.repo,
                Some(&task.id),
                &truncate_chars(&report, crate::task::notes::overseer::MAX_TEXT_CHARS),
                crate::clock::now_ms(),
            )
            .with_detail(&report)
            .with_decision(&[key]),
        );
        if self.settings.read().unwrap().auto_answer {
            let prompt = crate::prompts::get("overseer_failed")
                .replace("{id}", &task.id)
                .replace("{title}", &task.title);
            crate::engine::chat::wake::wake_overseer(
                self,
                &task.repo,
                crate::engine::chat::wake::WakeRequest::new(
                    prompt,
                    false,
                    "failed",
                    Some(&task.id),
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::attempt_with_failure;

    fn timed_out_attempt(n: u32, signature: &str) -> Attempt {
        let mut a = attempt_with_failure(n, signature);
        a.verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: None,
            tail: "killed after the time limit".into(),
            ms: 1,
            load: 0.0,
            cores: 1,
            load_timeout: false,
            decisions: vec![],
            ..Default::default()
        }];
        a
    }

    #[test]
    fn timeouts_do_not_count_toward_the_same_failure_stop() {
        let mut slow = task_with_status(TaskStatus::Running);
        slow.fixture_data().attempts = (1..=3)
            .map(|n| timed_out_attempt(n, "verify:timeout"))
            .collect();
        // Three timeouts in a row: the environment, so the task goes on.
        assert!(advance_after_failure(&mut slow, 10), "{:?}", slow.question);
        assert!(slow.question.is_none());
        // The same three failures without such a signal stop the loop.
        let mut real = task_with_status(TaskStatus::Running);
        real.fixture_data().attempts = (1..=3)
            .map(|n| attempt_with_failure(n, "verify:timeout"))
            .collect();
        assert!(!advance_after_failure(&mut real, 10));
        assert_eq!(
            real.question.as_ref().map(|q| q.kind),
            Some(QuestionKind::AttemptsFailing)
        );
    }

    #[tokio::test]
    async fn the_end_of_the_ladder_fails_the_task_with_a_report_and_asks_nothing() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().title = "Ship the change".into();
        task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:boom")];
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let signals = TaskSignals::new();
        let mut budget = 1;
        let mut permit = None;

        // Budget spent: one changed approach, not a question.
        let signal = fail_and_continue(
            &app,
            &mut task,
            decision::failure(FailureKind::Verify, "boom".into()),
            &mut budget,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
        assert!(matches!(signal, LoopSignal::Continue));
        assert!(task.question.is_none());
        assert_eq!(task.tier, Tier::Hard, "a higher tier is defined");
        assert!(approach_pending(&task), "{:?}", task.decisions);
        assert_eq!(budget, 3);

        // The approach is already changed: the task fails.
        task.fixture_data()
            .attempts
            .push(attempt_with_failure(2, "verify:boom"));
        task.resume_phase(TaskStatus::Running, task.updated_at);
        budget = 2;
        let signal = fail_and_continue(
            &app,
            &mut task,
            decision::failure(FailureKind::Verify, "boom".into()),
            &mut budget,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
        assert!(matches!(signal, LoopSignal::Stop));
        assert_eq!(task.status, TaskStatus::Failed);
        assert!(task.question.is_none());
        let saved = app.store.load_task(&id).unwrap().unwrap();
        assert_eq!(saved.status, TaskStatus::Failed);
        assert!(saved.question.is_none());

        // One owner notice with the report, however often the loop ends.
        app.notify_task_failed(&saved);
        app.notify_task_failed(&saved);
        let notices: Vec<_> = app
            .notice_log()
            .into_iter()
            .filter(|e| e.task_id.as_deref() == Some(id.as_str()) && e.kind == "asked_owner")
            .collect();
        assert_eq!(notices.len(), 1, "{notices:?}");
        let report = notices[0].detail.clone().unwrap();
        assert!(report.contains("Ship the change"), "{report}");
        assert!(report.contains("changed approach"), "{report}");
    }

    #[test]
    fn a_task_that_did_not_fail_gets_no_notice() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Done);
        app.notify_task_failed(&task);
        task.resume_phase(TaskStatus::Failed, task.updated_at);
        task.fixture_data().parent = Some("parent".into());
        app.notify_task_failed(&task);
        assert!(app.notice_log().is_empty());
    }

    #[test]
    fn a_fallback_review_route_is_another_model_and_none_when_there_is_none() {
        let (app, _dir) = test_app();
        let mut settings = app.settings.read().unwrap().clone();
        let current = settings.routes[0].clone();
        let (alt, _) = fallback_review_route(&app, &settings, &current).expect("another route");
        assert!(
            alt.id != current.id && (alt.harness, &alt.model) != (current.harness, &current.model)
        );
        settings.routes.retain(|r| r.id == current.id);
        assert!(fallback_review_route(&app, &settings, &current).is_none());
    }
    fn broken_untouched_test(n: u32) -> Attempt {
        let mut a = attempt_with_failure(n, "verify:untouched test fails");
        a.changed_files = vec!["src/engine.rs".into()];
        a.verify = vec![VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "not ok 1 - tests/untouched.test.cjs\n".into(),
            ms: 1,
            load: 0.0,
            cores: 1,
            load_timeout: false,
            decisions: vec![],
            ..Default::default()
        }];
        a
    }

    #[test]
    fn a_change_that_breaks_an_untouched_test_three_times_still_stops_the_loop() {
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = (1..=3).map(broken_untouched_test).collect();
        // The signal is there (the failing file is outside the diff) ...
        assert!(!crate::task::attempts::signals::attempt_signals(&task, 2).is_empty());
        // ... but nothing backs it up: it is the approach, not the environment.
        assert!(!advance_after_failure(&mut task, 10));
        assert_eq!(
            task.question.as_ref().map(|q| q.kind),
            Some(QuestionKind::AttemptsFailing)
        );
        assert!(!failed_on_environment(&task, &|_| None));
    }

    #[test]
    fn an_outside_diff_failure_that_also_fails_on_the_base_is_the_environment() {
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = (1..=3).map(broken_untouched_test).collect();
        let base = |_: &str| Some("not ok 1 - tests/untouched.test.cjs\n".to_string());
        assert!(failed_on_environment(&task, &base));
        assert!(advance_after_failure_with(&mut task, 10, &base));
        assert!(task.question.is_none());
    }

    #[test]
    fn a_dependency_that_broke_an_untouched_test_is_not_restarted_by_itself() {
        let mut dep = task_with_status(TaskStatus::Failed);
        dep.fixture_data().attempts = (1..=3).map(broken_untouched_test).collect();
        assert!(!failed_on_environment(&dep, &|_| None));
    }

    #[tokio::test]
    async fn a_second_agent_retry_through_the_dependency_question_is_refused() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let repo = repo.display().to_string();
        let mut dep = task_with_status(TaskStatus::Failed);
        dep.fixture_data().id = "dep".into();
        dep.fixture_data().repo = repo.clone();
        let mut waiting = task_with_status(TaskStatus::Waiting);
        waiting.fixture_data().id = "waiting".into();
        waiting.fixture_data().repo = repo;
        waiting.fixture_data().depends_on = vec!["dep".into()];
        waiting.fixture_data().question = Some(Question::new(
            format!("{DEPENDENCY_QUESTION} dep"),
            vec!["retry the dependency".into(), "stop".into()],
            QuestionKind::DependencyEnded,
            AskedBy::Verify,
            crate::clock::now_ms(),
        ));
        app.store.save_task(&dep).unwrap();
        app.store.save_task(&waiting).unwrap();
        let retry = || {
            app.answer_dependency_question(
                waiting.clone(),
                &crate::engine::questions::reply::Reply {
                    text: ("retry the dependency").to_string(),
                    default_evidence: None,
                    like: None,
                },
                Some(AnsweredBy::Overseer),
            )
        };
        // The first agent retry goes through, and its record survives the requeue.
        retry().unwrap();
        let stored = app.store.load_task("dep").unwrap().unwrap();
        assert!(
            stored.loop_state.agent_restarted
                && stored.decisions.contains(&AGENT_RESTART_LINE.to_string()),
            "{:?}",
            stored.decisions
        );
        // Failed again: the second is refused.
        let mut failed = stored;
        failed.mark_failed();
        app.store.save_task(&failed).unwrap();
        let err = retry().unwrap_err();
        assert!(err.contains("only the owner"), "{err}");
        assert!(!app.restart_environment_dependency(
            &waiting,
            &app.repo_tasks(&waiting.repo),
            &["dep".to_string()]
        ));
        app.shutdown();
    }

    #[tokio::test]
    async fn a_parent_attempt_finishing_stays_unreviewed_and_lands_nothing() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);
        let mut parent = task_with_status(TaskStatus::Running);
        parent.fixture_data().id = "parent".into();
        parent.fixture_data().repo = repo.display().to_string();
        parent.fixture_data().worktree = parent.repo.clone();
        parent.fixture_data().variant = Some(Variant {
            land: true,
            ..Variant::default()
        });
        parent.fixture_data().attempts = vec![attempt_with_failure(1, "none")];
        let mut child = task_with_status(TaskStatus::Done);
        child.fixture_data().id = "child".into();
        child.fixture_data().repo = parent.repo.clone();
        child.fixture_data().parent = Some("parent".into());
        child.fixture_data().unreviewed = true;
        app.store.save_task(&child).unwrap();
        // The parent's own attempt had a verdict: its flag was reset.
        parent.fixture_data().unreviewed = false;
        app.store.save_task(&parent).unwrap();
        let mut permit = None;
        let signals = TaskSignals::new();
        let run_dir = dir.path().join("run");
        std::fs::create_dir(&run_dir).unwrap();
        let wt = repo.clone();
        AttemptFiles::new(0, 1, wt, run_dir)
            .finish(
                &app,
                &mut parent,
                &mut 3,
                &mut QuestionSlot::new(&signals, &mut permit),
            )
            .await;
        assert!(parent.unreviewed, "{:?}", parent.decisions);
        assert!(
            !parent.decisions.iter().any(|d| d.starts_with("Land:")),
            "{:?}",
            parent.decisions
        );
        assert!(parent.landed_sha.is_none());
        app.shutdown();
    }

    #[tokio::test]
    async fn a_task_that_fails_again_with_the_same_attempts_is_told_again() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:boom")];
        task.mark_failed();
        app.notify_task_failed(&task);
        app.notify_task_failed(&task);
        // A restart whose setup fails three times: Failed again, same attempts.
        task.resume_phase(TaskStatus::Running, task.updated_at);
        task.mark_failed();
        app.notify_task_failed(&task);
        let told = app
            .notice_log()
            .into_iter()
            .filter(|e| e.kind == "asked_owner" && e.task_id.as_deref() == Some(task.id.as_str()))
            .count();
        assert_eq!(told, 2);
    }

    #[tokio::test]
    async fn open_attempts_failing_and_no_verdict_questions_are_retired_on_start() {
        let (app, _dir) = test_app();
        let mut exhausted = task_with_status(TaskStatus::Waiting);
        exhausted.fixture_data().attempts = vec![attempt_with_failure(1, "verify:boom")];
        exhausted.fixture_data().question = Some(Question::new(
            "Attempts keep failing",
            vec!["continue".into(), "stop".into()],
            QuestionKind::AttemptsFailing,
            AskedBy::Implement,
            crate::clock::now_ms(),
        ));
        let mut silent = task_with_status(TaskStatus::Waiting);
        silent.fixture_data().id = "silent".into();
        let mut attempt = attempt_with_failure(1, "blocked:no verdict");
        attempt.status = AttemptStatus::Blocked;
        silent.fixture_data().attempts = vec![attempt];
        silent.fixture_data().question = Some(Question::new(
            "The review gave no verdict",
            vec!["approve".into()],
            QuestionKind::ReviewNoVerdict,
            AskedBy::Review,
            crate::clock::now_ms(),
        ));
        app.store.save_task(&exhausted).unwrap();
        app.store.save_task(&silent).unwrap();

        app.retire_ladder_questions();

        let exhausted = app.store.load_task(&exhausted.id).unwrap().unwrap();
        assert_eq!(exhausted.status, TaskStatus::Queued);
        assert!(exhausted.question.is_none() && approach_changed(&exhausted));
        let silent = app.store.load_task("silent").unwrap().unwrap();
        assert!(silent.question.is_none() && silent.unreviewed);
        assert_eq!(silent.status, TaskStatus::Queued);
        assert_eq!(pending_unreviewed(&silent), Some(0));
        // A decision line pushed later does not hide the pending attempt.
        let mut later = silent.clone();
        later
            .fixture_data()
            .decisions
            .push("Owner: looked at it".into());
        assert_eq!(pending_unreviewed(&later), Some(0));
        later.fixture_data().loop_state.unreviewed_attempt = None;
        assert_eq!(pending_unreviewed(&later), None);
    }

    #[test]
    fn the_changed_approach_is_a_field_not_a_decision_line() {
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:x")];
        assert!(!approach_changed(&task) && !approach_pending(&task));
        change_approach(&mut task, &Settings::default(), 1);
        assert!(approach_changed(&task) && approach_pending(&task));
        task.fixture_data().decisions.clear();
        assert!(approach_changed(&task) && approach_pending(&task));
    }

    #[test]
    fn the_resumed_planner_gets_a_continuation_not_the_parse_failure_brief() {
        let variant = Variant::default();
        let text = crate::engine::planning::prompt::build_plan_continue_brief(&variant, true);
        assert!(text.contains("cut off"), "{text}");
        assert!(!text.contains("did not contain"), "{text}");
    }

    #[tokio::test]
    async fn an_environment_failure_of_a_top_level_task_is_restarted_once_by_code() {
        let (app, _dir) = test_app();
        let mut task = task_with_status(TaskStatus::Failed);
        task.fixture_data().repo = std::env::temp_dir().display().to_string();
        task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:check")];
        task.fixture_data().attempts[0].stage = Stage::Implement;
        // Only the typed cause counts: the failure text says nothing.
        assert!(!restartable_on_environment(&task));
        task.fixture_data().attempts[0].cause = Some(Blame::Environment);
        assert!(restartable_on_environment(&task));
        app.store.save_task(&task).unwrap();
        let restarts = |app: &App| {
            app.store
                .load_task(&task.id)
                .unwrap()
                .unwrap()
                .decisions
                .iter()
                .filter(|d| *d == AGENT_RESTART_LINE)
                .count()
        };

        app.notify_task_failed(&task);
        for _ in 0..200 {
            if restarts(&app) == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(restarts(&app), 1);

        // Failed again: no second restart, the owner is told.
        let mut again = app.store.load_task(&task.id).unwrap().unwrap();
        again.mark_failed();
        app.store.save_task(&again).unwrap();
        assert!(!restartable_on_environment(&again));
        app.notify_task_failed(&again);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(restarts(&app), 1);
        app.shutdown();
    }
}
