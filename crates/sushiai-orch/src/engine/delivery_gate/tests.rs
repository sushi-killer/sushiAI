use super::{begin_delivery, DeliveryGate};
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::admission::CheckAdmission;
use crate::task::checks::authority::Origin;
use crate::task::{AttemptStatus, Stage, TaskStatus};
use serde_json::json;

#[tokio::test]
async fn delivery_admission_persists_landing_before_an_owner_rpc_can_queue_an_edit() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let expected = CheckAdmission::capture(&task);
    assert!(matches!(
        begin_delivery(&app, &mut task, &CancelToken::new(), &expected).unwrap(),
        DeliveryGate::Ready
    ));
    let before = serde_json::to_value(app.store.load_task(&task.id).unwrap().unwrap()).unwrap();
    let error = app
        .dispatch(
            "task.amend",
            json!({"id": task.id, "criteria": ["late edit"]}),
        )
        .await
        .unwrap_err();
    assert!(error.contains("landing"), "{error}");
    assert_eq!(
        serde_json::to_value(app.store.load_task(&task.id).unwrap().unwrap()).unwrap(),
        before
    );
}

#[tokio::test]
async fn a_pending_edit_defers_delivery_and_preserves_completed_attempt_receipts() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    let route = app.settings.read().unwrap().routes[0].clone();
    let index = crate::task::attempts::start(&mut task, Stage::Implement, &route, "fixture", 2);
    app.store.save_task(&task).unwrap();
    let expected = CheckAdmission::capture(&task);
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            verify: Some(vec!["new command".into()]),
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
    crate::task::attempts::end(
        &mut task,
        index,
        crate::task::attempts::AttemptEnd::Passed,
        9,
    );
    assert!(matches!(
        begin_delivery(&app, &mut task, &signals.cancel, &expected).unwrap(),
        DeliveryGate::Recheck
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Running);
    assert_eq!(saved.attempts[index].status, AttemptStatus::Passed);
    assert_eq!(saved.attempts[index].ended_at, Some(9));
    assert!(signals.pending_amend.lock().unwrap().is_some());
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}

#[test]
fn a_stale_delivery_snapshot_cannot_overwrite_a_new_plan_or_an_owner_stop() {
    let (app, _dir) = test_app();
    let mut stale = task_with_status(TaskStatus::Running);
    app.store.save_task(&stale).unwrap();
    let expected = CheckAdmission::capture(&stale);
    let mut current = stale.clone();
    current.fixture_data().criteria = vec!["current owner plan".into()];
    app.store.save_task(&current).unwrap();
    let before = serde_json::to_value(&current).unwrap();
    assert!(matches!(
        begin_delivery(&app, &mut stale, &CancelToken::new(), &expected).unwrap(),
        DeliveryGate::Recheck
    ));
    assert_eq!(
        serde_json::to_value(app.store.load_task(&stale.id).unwrap().unwrap()).unwrap(),
        before
    );
    current.interrupt(None, true, "Owner: stop".into(), 12);
    app.store.save_task(&current).unwrap();
    assert!(matches!(
        begin_delivery(&app, &mut stale, &CancelToken::new(), &expected).unwrap(),
        DeliveryGate::Stopped
    ));
    assert_eq!(
        app.store.load_task(&stale.id).unwrap().unwrap().status,
        TaskStatus::Stopped
    );
}

#[tokio::test]
async fn a_carry_publishes_its_base_without_losing_a_queued_owner_edit() {
    let (app, _dir) = test_app();
    let before = task_with_status(TaskStatus::Running);
    app.store.save_task(&before).unwrap();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &before,
        Amendment {
            verify: Some(vec!["owner-current-command".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ));
    let mut queued = app.store.load_task(&before.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = signals.pending_amend.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    app.controls.lock().unwrap().insert(
        before.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let mut carried = before.clone();
    crate::task::transitions::move_base(&mut carried, "new-base".into());
    assert!(matches!(
        super::publish_carry(&app, &carried, &before, &signals.cancel).unwrap(),
        DeliveryGate::Ready
    ));
    assert_eq!(
        app.store.load_task(&before.id).unwrap().unwrap().base_sha,
        "new-base"
    );
    let settings = app.settings.read().unwrap().clone();
    let pending = signals.pending_amend.lock().unwrap().clone().unwrap();
    let current = pending
        .preview(
            &carried,
            &crate::engine::answer_policy::authority::check_authority(&settings, &carried),
            3,
        )
        .unwrap();
    assert_eq!(current.verify, ["owner-current-command"]);
    let expected = CheckAdmission::capture(&carried);
    assert!(matches!(
        begin_delivery(&app, &mut carried, &signals.cancel, &expected).unwrap(),
        DeliveryGate::Recheck
    ));
    let control = app.controls.lock().unwrap().remove(&before.id).unwrap();
    control.handle.await.unwrap();
}

#[test]
fn an_async_carry_cannot_replace_a_newer_plan_or_restart_an_owner_stop() {
    let (app, _dir) = test_app();
    let before = task_with_status(TaskStatus::Running);
    let mut carried = before.clone();
    crate::task::transitions::move_base(&mut carried, "new-base".into());
    let mut current = before.clone();
    current
        .fixture_data()
        .planner_tests
        .files
        .push(crate::task::PlannedTest {
            path: "owner-test.rs".into(),
            content: "new owner content".into(),
        });
    app.store.save_task(&current).unwrap();
    let saved = serde_json::to_value(&current).unwrap();
    assert!(matches!(
        super::publish_carry(&app, &carried, &before, &CancelToken::new()).unwrap(),
        DeliveryGate::Recheck
    ));
    assert_eq!(
        serde_json::to_value(app.store.load_task(&before.id).unwrap().unwrap()).unwrap(),
        saved
    );
    current.interrupt(None, true, "Owner: stop".into(), 8);
    app.store.save_task(&current).unwrap();
    let stopped = serde_json::to_value(&current).unwrap();
    assert!(matches!(
        super::publish_carry(&app, &carried, &before, &CancelToken::new()).unwrap(),
        DeliveryGate::Stopped
    ));
    assert_eq!(
        serde_json::to_value(app.store.load_task(&before.id).unwrap().unwrap()).unwrap(),
        stopped
    );
}

#[test]
fn queued_preparation_can_publish_a_carry_without_authorizing_delivery() {
    let (app, _dir) = test_app();
    let before = task_with_status(TaskStatus::Queued);
    app.store.save_task(&before).unwrap();
    let mut carried = before.clone();
    crate::task::transitions::move_base(&mut carried, "new-base".into());
    assert!(matches!(
        super::publish_carry(&app, &carried, &before, &CancelToken::new()).unwrap(),
        DeliveryGate::Ready
    ));
    let expected = CheckAdmission::capture(&carried);
    assert!(matches!(
        begin_delivery(&app, &mut carried, &CancelToken::new(), &expected).unwrap(),
        DeliveryGate::Stopped
    ));
    assert_eq!(
        app.store.load_task(&before.id).unwrap().unwrap().status,
        TaskStatus::Queued
    );
}
