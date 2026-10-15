use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::Origin;
use crate::task::TaskStatus;

#[tokio::test]
async fn a_queued_dependency_edit_wins_before_an_implementation_attempt_starts() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            depends_on: Some(vec!["dependency".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ));
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let route = app.settings.read().unwrap().routes[0].clone();
    assert!(super::start(&app, &mut task, &route, "fixture", &[])
        .unwrap()
        .is_none());
    assert!(task.attempts.is_empty());
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .attempts
        .is_empty());
    assert!(signals.pending_amend.lock().unwrap().is_some());
    signals.pending_amend.lock().unwrap().take();
    let index = super::start(&app, &mut task, &route, "fixture", &[])
        .unwrap()
        .unwrap();
    assert_eq!(index, 0);
    assert_eq!(
        app.store
            .load_task(&task.id)
            .unwrap()
            .unwrap()
            .attempts
            .len(),
        1
    );
    let error = app
        .dispatch(
            "task.amend",
            serde_json::json!({"id": task.id, "dependsOn": []}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("implementing"), "{error}");
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}
