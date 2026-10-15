use crate::clock::now_ms;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::wait_sla::tests::support::park_live;
use crate::engine::wait_sla::tests::support::waiting_in;
use crate::engine::wait_sla::tests::support::T0;
use crate::policy::routing::Harness;
use crate::task::QuestionKind;
use crate::task::TaskStatus;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

#[tokio::test]
async fn a_daily_budget_wait_resumes_once_todays_spend_is_under_the_budget() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().daily_budget_usd = 5.0;
    let task = waiting_in(&repo, QuestionKind::DailyBudget, &["run anyway", "stop"]);
    app.store.save_task(&task).unwrap();
    let waiter = park_live(&app, task).await;
    app.wait_sla_tick(T0).await;
    let (answer, task) = waiter.await.unwrap();
    assert_eq!(answer.as_deref(), Ok("retry"));
    assert!(task.decisions.iter().any(|d| d.starts_with("Policy:")));
    assert!(task.daily_budget_ok_day.is_none());
}

#[tokio::test]
async fn a_daily_budget_wait_keeps_waiting_while_todays_spend_is_at_the_budget() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    app.settings.write().unwrap().daily_budget_usd = 5.0;
    crate::store::costs::append(
        &app.data_dir,
        &crate::store::costs::CostRecord {
            ts: T0,
            repo: "/r".into(),
            task_id: None,
            run_id: None,
            stage: "implement".into(),
            attempt: None,
            route_id: "r".into(),
            harness: Harness::Claude,
            model: "m".into(),
            cost_usd: 6.0,
            estimated: false,
            input_tokens: 0,
            cached_tokens: 0,
            output_tokens: 0,
            ms: 0,
            backfilled: false,
        },
    );
    let task = waiting_in(&repo, QuestionKind::DailyBudget, &["run anyway", "stop"]);
    app.store.save_task(&task).unwrap();
    app.wait_sla_tick(T0).await;
    let task = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Waiting);
}

/// Sets one variable for the test and removes it again, even when the
/// test panics; tests that set it run one at a time.
struct VarGuard(
    &'static str,
    #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
);

impl VarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        static LOCK: StdMutex<()> = StdMutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(key, value);
        Self(key, guard)
    }
}

impl Drop for VarGuard {
    fn drop(&mut self) {
        std::env::remove_var(self.0);
    }
}

#[tokio::test]
async fn a_harness_wait_resumes_once_a_cli_can_be_started_and_only_once() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let _cli = VarGuard::set("ORCHD_CODEX_BIN", "/bin/sh");
    let task = waiting_in(&repo, QuestionKind::HarnessMissing, &["retry"]);
    app.store.save_task(&task).unwrap();
    let waiter = park_live(&app, task.clone()).await;
    app.wait_sla_tick(T0).await;
    let (answer, _) = waiter.await.unwrap();
    assert_eq!(answer.as_deref(), Ok("retry"));
    // The spawn fails again and the owner is asked again: no new resume.
    let mut again = task;
    again.fixture_data().status = TaskStatus::Waiting;
    app.store.save_task(&again).unwrap();
    let waiter = park_live(&app, again.clone()).await;
    app.wait_sla_tick(T0 + 60_000).await;
    let mut waiter = waiter;
    let resumed = tokio::time::timeout(Duration::from_millis(300), &mut waiter).await;
    assert!(resumed.is_err(), "the second wait is not resumed");
    let saved = app.store.load_task(&again.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    waiter.abort();
}

#[tokio::test]
async fn a_pause_that_outlives_the_loop_is_served_after_a_restart_and_then_cleared() {
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().repo = dir.path().join("gone").to_string_lossy().to_string();
    task.fixture_data().worktree = dir.path().join("gone-wt").to_string_lossy().to_string();
    let until = now_ms() + 700;
    task.fixture_data().queue.paused_until = Some(until);
    app.store.save_task(&task).unwrap();
    app.spawn_task_loop(task.id.clone(), true);
    // The property is that the pause is served in full and then cleared. A wall-clock
    // sample of "still paused" would race the loop on a loaded machine: check the
    // moment of clearing against the pause's own end instead.
    let end = std::time::Instant::now() + crate::procs::testing::deadline();
    loop {
        let t = app.store.load_task(&task.id).unwrap().unwrap();
        if t.queue.paused_until.is_none() {
            assert!(now_ms() >= until, "the pause was cleared before it ended");
            return;
        }
        assert!(
            std::time::Instant::now() < end,
            "the pause was never cleared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
