use super::*;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::Origin;
use crate::task::{AskedBy, QuestionKind};

fn task() -> Task {
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["Keep the API".into()];
    task.fixture_data().attempts = vec![attempt_with_failure(1, "verify:failed")];
    task
}

fn ask(task: &mut Task) {
    task.ask(
        Question::new(
            "Can this be met?",
            vec!["retry".into(), "stop".into()],
            QuestionKind::Impossible,
            AskedBy::Implement,
            1,
        ),
        1,
    );
}

fn ruling() -> AnswerRuling {
    AnswerRuling {
        answer: Some(AutomaticAnswer::Judge {
            text: "retry".into(),
            evidence: "verified".into(),
        }),
        rewrite: None,
        cost: 0.01,
        notes: vec!["Answer judge: observed evidence".into()],
    }
}

#[test]
fn accepted_policy_publishes_only_its_delta_and_retains_unpublished_native_facts() {
    let (app, _dir) = test_app();
    let mut task = task();
    task.fixture_data().queue.rate_pauses = 2;
    app.store.save_task(&task).unwrap();
    task.reset_rate_pauses();
    ask(&mut task);
    task.fixture_data().attempts[0].summary = Some("current native receipt B".into());
    task.charge(0.4);
    let captured = PolicyInput::capture(&app, &task).unwrap();
    let result = publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap();
    assert!(matches!(result, PolicyResult::Answered(ref text) if text == "retry"));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.cost_usd, 0.01);
    assert_eq!(saved.assumptions.len(), 1);
    assert_eq!(saved.question_history.len(), 1);
    assert!(saved.question.is_none());
    assert_eq!(saved.status, TaskStatus::Queued);
    assert_eq!(
        task.attempts[0].summary.as_deref(),
        Some("current native receipt B")
    );
    assert!((task.cost_usd - 0.41).abs() < 1e-9);
    assert_eq!(
        task.queue.rate_pauses, 0,
        "native completion already cleared the rate pauses"
    );
    assert_eq!(task.question_history, saved.question_history);
    assert_eq!(task.assumptions, saved.assumptions);
    crate::store::precedents::forget_synced();
    app.store.save_task(&task).unwrap();
    let (memories, _) = crate::store::precedents::list(&app.data_dir, &task.repo);
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].count, 1);
}

#[test]
fn a_stopped_or_changed_owner_contract_discards_the_answer_but_settles_actual_spend() {
    for stopped in [true, false] {
        let (app, _dir) = test_app();
        let mut task = task();
        app.store.save_task(&task).unwrap();
        ask(&mut task);
        task.fixture_data().attempts[0].summary = Some("current native receipt B".into());
        let captured = PolicyInput::capture(&app, &task).unwrap();
        let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
        if stopped {
            fresh.resume_phase(TaskStatus::Stopped, 2);
        } else {
            fresh.fixture_data().criteria = vec!["Owner's new criterion".into()];
        }
        app.store.save_task(&fresh).unwrap();
        let result = publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap();
        assert!(matches!(
            (stopped, result),
            (true, PolicyResult::Stopped) | (false, PolicyResult::Stale)
        ));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.criteria, fresh.criteria);
        assert_eq!(saved.status, fresh.status);
        assert_eq!(saved.cost_usd, 0.01);
        assert!(saved.assumptions.is_empty());
        assert!(saved.question_history.is_empty());
        if !stopped {
            assert_eq!(
                task.attempts[0].summary.as_deref(),
                Some("current native receipt B")
            );
        }
    }
}

#[tokio::test]
async fn an_accepted_pending_amendment_survives_policy_publication_and_keeps_native_receipt() {
    let (app, _dir) = test_app();
    let mut task = task();
    app.store.save_task(&task).unwrap();
    ask(&mut task);
    task.fixture_data().attempts[0].summary = Some("current native receipt B".into());
    let captured = PolicyInput::capture(&app, &task).unwrap();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            criteria: Some(vec!["Owner amendment".into()]),
            ..Default::default()
        },
        Origin::Owner,
    ));
    let mut queued = app.store.load_task(&task.id).unwrap().unwrap();
    queued.fixture_data().pending_amendment = signals.pending_amend.lock().unwrap().clone();
    app.store.save_task_with_pending(&queued, None).unwrap();
    let pending = signals.pending_amend.clone();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals,
            handle: tokio::spawn(async {}),
        },
    );
    let result = publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap();
    assert!(matches!(result, PolicyResult::Stale));
    assert!(pending.lock().unwrap().is_some());
    assert_eq!(
        task.attempts[0].summary.as_deref(),
        Some("current native receipt B")
    );
    assert_eq!(task.cost_usd, 0.01);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["Keep the API"]);
    assert!(saved.assumptions.is_empty());
    assert_eq!(saved.cost_usd, 0.01);
}

#[tokio::test]
async fn an_owner_overturn_while_the_answer_judge_is_open_survives_the_parking_caller() {
    use crate::engine::questions::parking::QuestionSlot;
    let (app, _dir) = test_app();
    app.settings.write().unwrap().orchestrator.clear();
    let mut local = task();
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
    ask(&mut local);
    local.fixture_data().attempts[0].summary = Some("paid native receipt".into());
    let input = PolicyInput::capture(&app, &local).unwrap();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let worker_app = app.clone();
    let worker = tokio::spawn(async move {
        blocked.await.unwrap();
        let result = publish(
            &worker_app,
            &mut local,
            input,
            AnswerRuling {
                answer: None,
                rewrite: None,
                cost: 0.25,
                notes: vec!["judge completed".into()],
            },
            &CancelToken::new(),
        )
        .unwrap();
        if matches!(result, PolicyResult::Open) {
            let signals = TaskSignals::new();
            let mut permit = None;
            let mut slot = QuestionSlot::new(&signals, &mut permit);
            let future = slot.park(&worker_app, &mut local);
            tokio::pin!(future);
            crate::engine::persistence::contract::tests::pending_once(future.as_mut()).await;
        }
        (result, local)
    });
    app.dispatch("task.overturn",serde_json::json!({"id":app.store.list_tasks().unwrap()[0].id,"index":0,"answer":"Keep this check"})).await.unwrap();
    release.send(()).unwrap();
    let (result, local) = worker.await.unwrap();
    let saved = app.store.load_task(&local.id).unwrap().unwrap();
    assert!(
        saved.assumptions[0].overturned,
        "parking restored the revoked waiver"
    );
    assert_eq!(
        saved.assumptions[0].owner_answer.as_deref(),
        Some("Keep this check")
    );
    assert!(matches!(result, PolicyResult::Stale));
    assert_eq!(saved.cost_usd, 0.25);
    assert!(saved.question.is_none());
    assert_eq!(
        local.attempts[0].summary.as_deref(),
        Some("paid native receipt")
    );
}
