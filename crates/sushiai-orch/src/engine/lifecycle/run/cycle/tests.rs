use super::{execute, native_result, NativeEnd, Receipt};
use crate::clock::now_ms;
use crate::engine::attempts::best_of::outcome::BestOfExtra;
use crate::engine::lifecycle::completion::Tail;
use crate::engine::lifecycle::execution::{ExecutedAttempt, RunReceipt};
use crate::engine::lifecycle::round::LoopProgress;
use crate::engine::lifecycle::run::end::CycleEnd;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::engine::verification::base::amendments::apply_pending_amendment;
use crate::harness::RunOutcome;
use crate::policy::pipelines::task::{compiled, TaskStep};
use crate::policy::settings::SandboxMode;
use crate::task::{attempts, Stage, Task, TaskStatus};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn completed_run() -> (Arc<App>, tempfile::TempDir, Task, ExecutedAttempt, PathBuf) {
    let (app, home) = test_app();
    let repo = home.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["checkout", "-qb", "task/flow"]);
    let trace = home.path().join("operations");
    let mut task = task_with_status(TaskStatus::Running);
    let settings = {
        let mut settings = app.settings.write().unwrap();
        settings.sandbox = SandboxMode::Host;
        settings.review.clear();
        settings.pr_on_done_off = true;
        settings.clone()
    };
    let route = settings
        .routes
        .iter()
        .find(|route| route.id == "claude-sonnet")
        .unwrap()
        .clone();
    {
        let data = task.fixture_data();
        data.repo = repo.display().to_string();
        data.worktree = data.repo.clone();
        data.branch = "task/flow".into();
        data.base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
        data.variant.get_or_insert_default().land = false;
        data.verify = vec![format!("printf 'checks\\n' >> '{}'", trace.display())];
        data.final_verify = vec![format!("printf 'final\\n' >> '{}'", trace.display())];
    }
    let idx = attempts::start(
        &mut task,
        Stage::Implement,
        &route,
        "flow fixture",
        now_ms(),
    );
    let run_dir = app.store.run_dir(&task.id, 1);
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(repo.join("result.txt"), "implemented\n").unwrap();
    std::fs::write(run_dir.join("events.raw"), "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"flow\"}\n{\"type\":\"result\",\"result\":\"Implemented fixture.\",\"total_cost_usd\":0.2}\n").unwrap();
    std::fs::write(run_dir.join("events.exit"), "0\n").unwrap();
    app.store.save_task(&task).unwrap();
    let executed = ExecutedAttempt {
        run: RunReceipt {
            settings,
            attempt_n: 1,
            idx,
            route,
            worktree: repo,
            base_sha: task.base_sha.clone(),
            run_dir: run_dir.clone(),
        },
        key_path: run_dir.join("key"),
        registered: None,
        run_result: Ok(RunOutcome {
            final_text: Some("Implemented fixture.".into()),
            cost_usd: Some(0.2),
            ..Default::default()
        }),
        best_of: BestOfExtra::default(),
    };
    (app, home, task, executed, trace)
}

#[tokio::test]
async fn a_completed_native_run_follows_yaml_through_checks_and_delivery() {
    let (app, _home, mut task, _executed, trace) = completed_run();
    assert_eq!(
        crate::engine::recovery::resumable_attempt(&app, &task),
        Some(0)
    );
    let signals = TaskSignals::new();
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        execute(
            &app,
            &mut task,
            &signals,
            &mut None,
            &mut LoopProgress::new(1, true),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Ok(Tail::Return)));
    assert_eq!(std::fs::read_to_string(trace).unwrap(), "checks\nfinal\n");
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.attempts.len(), 1);
    assert_eq!(task.attempts[0].cost_usd, Some(0.2));
    assert_eq!(
        app.store.load_task(&task.id).unwrap().unwrap().status,
        TaskStatus::Done
    );
}

#[tokio::test]
async fn changed_native_publication_routes_to_retry_without_running_checks() {
    let (app, _home, mut task, executed, trace) = completed_run();
    let signals = TaskSignals::new();
    let (finish, waiting) = tokio::sync::oneshot::channel::<()>();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async move {
                let _ = waiting.await;
            }),
        },
    );
    let amended = app
        .dispatch(
            "task.amend",
            json!({
                "id": task.id, "criteria": ["The owner's replacement criterion"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(amended["pending"], true);
    let queued = app.store.load_task(&task.id).unwrap().unwrap();
    let pending = queued
        .pending_amendment
        .clone()
        .expect("acknowledged edit is durable");
    assert_eq!(
        signals.pending_amend.lock().unwrap().as_ref(),
        Some(&pending)
    );
    assert_eq!(queued.criteria, task.criteria);
    let result = native_result::settle(&app, &mut task, executed);
    let Err(end) = result else {
        panic!("pending owner amendment continued into checks")
    };
    assert!(matches!(
        CycleEnd::from(end).route(compiled(), TaskStep::Settle),
        CycleEnd::Changed
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.pending_amendment.as_ref(), Some(&pending));
    assert_eq!(
        signals.pending_amend.lock().unwrap().as_ref(),
        Some(&pending)
    );
    assert_eq!(saved.cost_usd, 0.2);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.2));
    assert!(!trace.exists());

    apply_pending_amendment(&app, &mut task, &signals.pending_amend, &signals.cancel)
        .await
        .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["The owner's replacement criterion"]);
    assert!(saved.pending_amendment.is_none());
    assert!(signals.pending_amend.lock().unwrap().is_none());
    assert_eq!(saved.cost_usd, 0.2);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.2));
    finish.send(()).unwrap();
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}

#[tokio::test]
async fn stopped_native_publication_remains_terminal_and_preserves_paid_facts() {
    let (app, _home, mut task, executed, trace) = completed_run();
    let mut fresh = task.clone();
    fresh.resume_phase(TaskStatus::Stopped, now_ms());
    app.store.save_task(&fresh).unwrap();
    let result = Receipt::Native(Box::new(executed))
        .settle(&app, &mut task, &TaskSignals::new())
        .await;
    let Err(end) = result else {
        panic!("stopped native input continued")
    };
    assert!(matches!(
        end.route(compiled(), TaskStep::Settle),
        CycleEnd::Stopped
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert_eq!(saved.cost_usd, 0.2);
    assert!(!trace.exists());
}

/// A writer put back a copy saved before the attempt started (the stale check decision of
/// CI run 38974451190): the run's result has no attempt to land on. The loop must not end
/// silently with the task left queued: the attempt fails visibly, and the failure path saves
/// the loop's copy, attempt included, so the ladder goes on.
#[tokio::test]
async fn a_result_whose_stored_attempt_vanished_fails_the_attempt_visibly() {
    let (app, _home, mut task, executed, trace) = completed_run();
    let mut stale = task.clone();
    stale.fixture_data().attempts.clear();
    stale.resume_phase(TaskStatus::Queued, now_ms());
    app.store.save_task(&stale).unwrap();
    let result = Receipt::Native(Box::new(executed))
        .settle(&app, &mut task, &TaskSignals::new())
        .await;
    let Err(end) = result else {
        panic!("a result without its stored attempt continued")
    };
    let CycleEnd::Failed(failure) = end.route(compiled(), TaskStep::Settle) else {
        panic!("the vanished attempt ended the loop without a failure")
    };
    assert_eq!(failure.kind, crate::task::FailureKind::Error);
    assert!(
        failure
            .detail
            .contains("could not save the result of attempt 1"),
        "{}",
        failure.detail
    );
    let signals = TaskSignals::new();
    let mut permit = None;
    let mut budget = 3;
    let signal = crate::engine::decision::fail_and_continue(
        &app,
        &mut task,
        failure,
        &mut budget,
        &mut crate::engine::questions::parking::QuestionSlot::new(&signals, &mut permit),
    )
    .await;
    assert!(matches!(
        signal,
        crate::engine::decision::LoopSignal::Continue
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.attempts.len(), 1, "the attempt is back in the store");
    assert!(saved.attempts[0]
        .failure
        .as_ref()
        .is_some_and(|f| f.detail.contains("could not save the result")));
    assert!(!trace.exists());
}

#[tokio::test]
async fn pending_owner_edit_keeps_the_actual_timeout_receipt_without_raw_adoption() {
    let (app, _home, mut task, mut executed, trace) = completed_run();
    let Ok(outcome) = executed.run_result.as_mut() else {
        panic!("fixture has no native outcome")
    };
    outcome.session_id = Some("native-timeout-session".into());
    outcome.time_budget = true;
    outcome.error = Some("native time budget exhausted".into());
    std::fs::remove_file(executed.run.run_dir.join("events.exit")).unwrap();
    let signals = TaskSignals::new();
    let (finish, waiting) = tokio::sync::oneshot::channel::<()>();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async move {
                let _ = waiting.await;
            }),
        },
    );
    let amended = app
        .dispatch(
            "task.amend",
            json!({
                "id": task.id, "criteria": ["The current Owner contract"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(amended["pending"], true);
    let result = Receipt::Native(Box::new(executed))
        .settle(&app, &mut task, &signals)
        .await;
    let Ok(Receipt::Observed(observed)) = result else {
        panic!("pending edit lost the actual native receipt")
    };
    assert!(observed.outcome.time_budget);
    assert_eq!(
        observed.outcome.session_id.as_deref(),
        Some("native-timeout-session")
    );
    assert_eq!(task.criteria, ["The current Owner contract"]);
    assert!(task.pending_amendment.is_none());
    assert_eq!(task.cost_usd, 0.2);
    assert_eq!(task.attempts[0].cost_usd, Some(0.2));
    let interruption = crate::engine::lifecycle::changes::capture_run_interruption(
        &mut task,
        &observed.run,
        &observed.outcome,
    )
    .await
    .unwrap();
    assert_eq!(interruption.0, crate::task::FailureKind::TimeBudget);
    assert!(!trace.exists());
    finish.send(()).unwrap();
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}

#[tokio::test]
async fn owner_stop_after_changed_native_publication_keeps_receipt_and_pending_edit() {
    let (app, _home, mut task, executed, trace) = completed_run();
    let signals = TaskSignals::new();
    let (finish, waiting) = tokio::sync::oneshot::channel::<()>();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async move {
                let _ = waiting.await;
            }),
        },
    );
    let amended = app
        .dispatch(
            "task.amend",
            json!({
                "id": task.id, "criteria": ["An edit pending before stop"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(amended["pending"], true);
    let Err(NativeEnd::Changed(observed)) = native_result::settle(&app, &mut task, executed) else {
        panic!("expected a retained native receipt")
    };
    app.dispatch("task.stop", json!({"id": task.id}))
        .await
        .unwrap();
    let stopped = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(matches!(
        observed.after_amendments(&app, &mut task, &signals).await,
        Err(NativeEnd::Stopped)
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert_eq!(saved.criteria, stopped.criteria);
    assert_eq!(saved.pending_amendment, stopped.pending_amendment);
    assert_eq!(saved.cost_usd, 0.2);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.2));
    assert!(!trace.exists());
    finish.send(()).unwrap();
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}
