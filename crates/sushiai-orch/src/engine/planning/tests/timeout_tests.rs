use crate::engine::planning::correction::PlannerCorrection;
use crate::engine::planning::correction::Uncorrected;
use crate::engine::planning::run::run_plan_stage;
use crate::engine::planning::run::PlanOutcome;
use crate::engine::run_control::TaskSignals;
#[cfg(test)]
use crate::engine::test_support::assert_helper_timeout;
#[cfg(test)]
use crate::engine::test_support::slow_helper_task;
use crate::harness::guard::helper_timeout_message;
use crate::task::FailureKind;
use crate::task::Stage;
use crate::task::TaskStatus;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::test]
async fn planner_timed_out_is_tried_once_more_with_twice_the_time_then_fails() {
    let (app, _dir, mut task, _) = slow_helper_task();
    task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
    task.fixture_data().attempts.clear();
    task.fixture_data().request = Some("Ship the change".into());
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    let mut permit = None;
    let result = run_plan_stage(&app, &task.id, &signals, &mut permit, true).await;
    assert!(matches!(result, PlanOutcome::Ended));
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(
        task.attempts[0].failure.as_ref().unwrap().kind,
        FailureKind::Error
    );
    // The retry ran with twice the limit and timed out too.
    assert_eq!(
        task.attempts[0].failure.as_ref().unwrap().detail,
        "planner timed out after 0.5s"
    );
    let first = helper_timeout_message("planner", Duration::from_millis(250));
    let second = helper_timeout_message("planner", Duration::from_millis(500));
    for line in [first, second] {
        assert_eq!(
            task.decisions.iter().filter(|d| **d == line).count(),
            1,
            "{:?}",
            task.decisions
        );
    }
    assert!(
        task.decisions
            .iter()
            .any(|d| d.contains("trying once more, resumed, with twice the time")),
        "{:?}",
        task.decisions
    );
    // Never a third run.
    assert!(crate::engine::chat::test_support::fake_overseer_calls(Path::new(&task.worktree)) <= 2);
}

#[tokio::test]
async fn planner_limit_is_the_setting_and_the_retry_doubles_it() {
    let (app, _dir, mut task, _) = slow_helper_task();
    app.plan_timeout_override_ms.store(0, Ordering::SeqCst);
    assert_eq!(app.planner_timeout(), Duration::from_secs(1800));
    app.settings.write().unwrap().plan_timeout_secs = 0;
    assert_eq!(app.planner_timeout(), Duration::from_secs(1800));
    app.settings.write().unwrap().plan_timeout_secs = 1;
    assert_eq!(app.planner_timeout(), Duration::from_secs(1));
    task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
    task.fixture_data().attempts.clear();
    task.fixture_data().request = Some("Ship the change".into());
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    let mut permit = None;
    let _ = run_plan_stage(&app, &task.id, &signals, &mut permit, true).await;
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(
        task.attempts[0].failure.as_ref().unwrap().detail,
        "planner timed out after 2s"
    );
}

#[tokio::test]
async fn planner_correction_timed_out_fails_instead_of_redrafting() {
    let (app, _dir, mut task, route) = slow_helper_task();
    task.resume_phase(TaskStatus::Drafting, crate::clock::now_ms());
    task.fixture_data().attempts[0].stage = Stage::Plan;
    task.fixture_data().attempts[0].session_id = Some("s".into());
    app.store.save_task(&task).unwrap();
    let correction = PlannerCorrection::resume(&app, &task.id, 0, &route)
        .unwrap_or_else(|_| panic!("the saved planner session must resume"));
    let result = correction.run(&app, "conflict", &TaskSignals::new()).await;
    assert!(matches!(result, Err(Uncorrected::Failed)));
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(
        task.attempts[1].failure.as_ref().unwrap().kind,
        FailureKind::Error
    );
    assert_ne!(task.attempts[0].n, task.attempts[1].n);
    assert_helper_timeout(&task, "planner");
}
