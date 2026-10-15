use super::*;
use crate::engine::run_control::TaskControl;
use crate::engine::test_support::{task_with_status, test_app};
use serde_json::json;

#[tokio::test]
async fn owner_start_clears_a_live_pause_and_wakes_it_without_waiting_for_the_deadline() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().queue.paused_until = Some(now_ms() + 3_600_000);
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let waiting = tokio::spawn({
        let app = app.clone();
        let signals = signals.clone();
        async move {
            let result = wait(&app, &mut task, &signals).await.unwrap();
            (result, task)
        }
    });
    tokio::task::yield_now().await;
    for _ in 0..2 {
        app.dispatch("task.start", json!({"id":id})).await.unwrap();
    }
    let (result, local) = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, PauseWait::Ready);
    assert_eq!(local.queue.paused_until, None);
    assert_eq!(
        app.store
            .load_task(&id)
            .unwrap()
            .unwrap()
            .queue
            .paused_until,
        None
    );
    assert!(!signals.cancel.is_cancelled());
}

#[tokio::test]
async fn notification_before_the_wait_is_observed_and_owner_fields_are_reloaded() {
    let (app, _dir) = test_app();
    let mut old = task_with_status(TaskStatus::Queued);
    old.fixture_data().queue.paused_until = Some(now_ms() + 3_600_000);
    app.store.save_task(&old).unwrap();
    let signals = TaskSignals::new();
    app.update_task(&old.id, |fresh| {
        fresh.clear_limit_pause();
        fresh.fixture_data().goal = "Owner changed the goal".into();
    })
    .unwrap();
    signals.resume.notify_one();
    assert_eq!(
        wait(&app, &mut old, &signals).await.unwrap(),
        PauseWait::Ready
    );
    assert_eq!(old.goal, "Owner changed the goal");
    assert_eq!(old.queue.paused_until, None);
}

#[tokio::test]
async fn cancellation_wins_over_a_ready_resume_notification() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().queue.paused_until = Some(now_ms() + 3_600_000);
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    signals.resume.notify_one();
    signals.cancel.cancel();
    assert_eq!(
        wait(&app, &mut task, &signals).await.unwrap(),
        PauseWait::Stopped
    );
}
