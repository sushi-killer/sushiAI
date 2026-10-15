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
    let mut queued = app.store.load_task(&task.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = signals.pending_amend.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let route = app.settings.read().unwrap().routes[0].clone();
    assert!(super::start(&app, &mut task, &route, "fixture")
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
    let acknowledged = signals.pending_amend.lock().unwrap().take().unwrap();
    assert!(super::start(&app, &mut task, &route, "fixture")
        .unwrap()
        .is_none());
    *signals.pending_amend.lock().unwrap() = Some(acknowledged);
    crate::engine::verification::base::amendments::apply_pending_amendment(
        &app,
        &mut task,
        &signals.pending_amend,
        &signals.cancel,
    )
    .await
    .unwrap();
    assert_eq!(task.depends_on, ["dependency"]);
    let index = super::start(&app, &mut task, &route, "fixture")
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

#[tokio::test]
async fn an_owner_overturn_after_preflight_prevents_the_stale_attempt_save() {
    let (app, _dir) = test_app();
    let mut local = task_with_status(TaskStatus::Queued);
    local.record_assumption(crate::task::Assumption {
        question: "Accept the inherited check?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same named failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&local).unwrap();
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        local.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(async {}),
        },
    );
    let id = local.id.clone();
    let worker_app = app.clone();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        blocked.await.unwrap();
        let route = worker_app.settings.read().unwrap().routes[0].clone();
        let result =
            super::start(&worker_app, &mut local, &route, "after preflight await").unwrap();
        (result, local)
    });
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id":id,"index":0,"answer":"Keep this check"}),
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    let (result, _local) = worker.await.unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert!(
        saved.assumptions[0].overturned,
        "attempt admission restored the revoked waiver"
    );
    assert!(result.is_none());
    assert!(saved.attempts.is_empty());
    assert_eq!(saved.status, TaskStatus::Queued);
}
