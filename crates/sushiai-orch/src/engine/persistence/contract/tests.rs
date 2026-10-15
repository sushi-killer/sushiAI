use super::*;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::amendment::storage::Settlement;
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::{CheckAuthority, Origin};
use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

pub(in crate::engine) fn queue_owner(app: &App, task: &Task) -> PendingAmendment {
    let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
    let before = fresh.pending_amendment.clone();
    fresh
        .queue_pending_amendment(
            Amendment {
                criteria: Some(vec!["Owner's accepted criterion".into()]),
                ..Default::default()
            },
            Origin::Owner,
            &CheckAuthority::default(),
            now_ms(),
        )
        .unwrap();
    app.store
        .save_task_with_pending(&fresh, before.as_ref())
        .unwrap();
    fresh.pending_amendment.clone().unwrap()
}

pub(in crate::engine) fn assert_owner_applies(app: &App, task: &Task) {
    let mut saved = app.store.load_task(&task.id).unwrap().unwrap();
    let receipt = saved.pending_amendment.clone().unwrap();
    assert!(matches!(
        saved.settle_pending_amendment(&CheckAuthority::default(), now_ms()),
        Settlement::Applied
    ));
    app.store
        .save_task_with_pending(&saved, Some(&receipt))
        .unwrap();
    let reloaded = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(reloaded.criteria, ["Owner's accepted criterion"]);
    assert!(reloaded.pending_amendment.is_none());
}

pub(in crate::engine) async fn pending_once<F: Future>(mut future: Pin<&mut F>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn an_owner_receipt_accepted_while_a_helper_waits_keeps_its_contract() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let (worker_app, observed) = (app.clone(), task.clone());
    let worker = tokio::spawn(async move {
        ready.send(()).unwrap();
        blocked.await.unwrap();
        input
            .publish(&worker_app, &observed, &CancelToken::new(), |fresh| {
                fresh.record_paths(vec!["helper-path".into()]);
                Ok(())
            })
            .unwrap()
            .into_result()
    });
    waiting.await.unwrap();
    queue_owner(&app, &task);
    release.send(()).unwrap();
    assert_eq!(worker.await.unwrap(), Err(WaitEnd::Changed));
    assert!(app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .paths
        .is_empty());
    assert_owner_applies(&app, &task);
}

#[tokio::test]
async fn live_pending_and_suspension_prevent_the_mutation_closure() {
    let (app, _dir) = test_app();
    let task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let mut pending = task.clone();
    pending
        .queue_pending_amendment(
            Amendment {
                criteria: Some(vec!["live".into()]),
                ..Default::default()
            },
            Origin::Owner,
            &CheckAuthority::default(),
            now_ms(),
        )
        .unwrap();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = pending.pending_amendment.clone();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(std::future::pending()),
        },
    );
    let input = ContractInput::capture(&task);
    let result = input
        .publish(
            &app,
            &task,
            &CancelToken::new(),
            |_| -> Result<(), String> { panic!("pending edit admitted") },
        )
        .unwrap();
    assert!(matches!(result, Publication::Deferred(WaitEnd::Changed)));
    app.controls
        .lock()
        .unwrap()
        .remove(&task.id)
        .unwrap()
        .handle
        .abort();
    for status in [TaskStatus::Stopped, TaskStatus::Done, TaskStatus::Failed] {
        app.update_task(&task.id, |fresh| fresh.fixture_data().status = status)
            .unwrap();
        assert!(matches!(
            input
                .publish(
                    &app,
                    &task,
                    &CancelToken::new(),
                    |_| -> Result<(), String> { panic!("terminal task admitted") }
                )
                .unwrap(),
            Publication::Deferred(WaitEnd::Stopped)
        ));
    }
}

#[test]
fn fresh_contract_phase_question_pause_and_cancel_are_checked() {
    let (app, _dir) = test_app();
    for change in 0..6 {
        let task = task_with_status(TaskStatus::Running);
        app.store.save_task(&task).unwrap();
        let input = ContractInput::capture(&task);
        let cancel = CancelToken::new();
        app.update_task(&task.id, |fresh| match change {
            0 => fresh.record_paths(vec!["Owner's path".into()]),
            1 => fresh.fixture_data().status = TaskStatus::Waiting,
            2 => fresh.fixture_data().queue.paused_until = Some(now_ms() + 60_000),
            3 => fresh.fixture_data().archived = true,
            4 => fresh.ask(
                crate::task::Question::new(
                    "Owner question",
                    vec![],
                    crate::task::QuestionKind::AgentQuestion,
                    crate::task::AskedBy::Implement,
                    now_ms(),
                ),
                now_ms(),
            ),
            _ => cancel.cancel(),
        })
        .unwrap();
        let end = input
            .publish(&app, &task, &cancel, |_| -> Result<(), String> {
                panic!("stale admission")
            })
            .unwrap()
            .into_result();
        assert_eq!(
            end,
            Err(if change == 3 || change == 5 {
                WaitEnd::Stopped
            } else {
                WaitEnd::Changed
            })
        );
    }
}

#[test]
fn accepted_publication_retains_fresh_observations_and_the_local_copy() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    task.record_decision("unsaved native observation".into());
    app.update_task(&task.id, |fresh| {
        fresh.charge(0.4);
        fresh.record_decision("fresh review observation".into());
    })
    .unwrap();
    let result = input
        .publish(&app, &task, &CancelToken::new(), |fresh| {
            fresh.record_paths(vec!["accepted".into()]);
            Ok(17)
        })
        .unwrap()
        .into_result();
    assert_eq!(result, Ok(17));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.cost_usd, 0.4);
    assert!(saved.decisions.contains(&"fresh review observation".into()));
    assert!(task
        .decisions
        .contains(&"unsaved native observation".into()));
    assert!(task.paths.is_empty());
}

#[tokio::test]
async fn an_owner_revocation_during_a_helper_result_prevents_contract_publication() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Running);
    task.record_assumption(crate::task::Assumption {
        question: "Accept the inherited check failure?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "Observed on the base".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&task).unwrap();
    let input = ContractInput::capture(&task);
    let (ready, waiting) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let (worker_app, observed) = (app.clone(), task.clone());
    let worker = tokio::spawn(async move {
        worker_app
            .update_task(&observed.id, |fresh| fresh.charge(0.25))
            .unwrap();
        ready.send(()).unwrap();
        blocked.await.unwrap();
        input
            .publish(&worker_app, &observed, &CancelToken::new(), |fresh| {
                fresh.record_paths(vec!["stale-helper-plan".into()]);
                Ok(())
            })
            .unwrap()
            .into_result()
    });
    waiting.await.unwrap();
    app.dispatch(
        "task.overturn",
        serde_json::json!({
            "id": task.id, "index": 0, "answer": "The check must pass"
        }),
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    assert_eq!(worker.await.unwrap(), Err(WaitEnd::Changed));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(saved.paths.is_empty());
    assert!(saved.assumptions[0].overturned);
    assert_eq!(saved.cost_usd, 0.25);
}
