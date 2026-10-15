use super::{publish, AnswerHealing, AnswerInput, AnswerRuling};
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::task::amendment::{Amendment, PendingAmendment};
use crate::task::checks::authority::Origin;
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::state::{Task, TaskStatus};

fn input(task: &Task) -> AnswerInput {
    AnswerInput {
        contract: CheckSnapshot::capture(task),
        phase: task.status,
        answers: crate::verify::healing::reply::new_answers(task),
        seen: task.brief_check.answers_seen,
        stored_cost: task.cost_usd,
    }
}
fn task() -> Task {
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().criteria = vec!["It is blue".into()];
    task.record_decision("Owner: Which color? -> red".into());
    task
}
fn ruling() -> AnswerRuling {
    AnswerRuling {
        amendments: vec![(0, "It is red".into())],
        cost: 0.01,
        timeout_note: None,
    }
}

#[test]
fn accepted_healing_is_stored_with_cost_and_progress_before_attempt_admission() {
    let (app, _dir) = test_app();
    let mut task = task();
    let captured = input(&task);
    app.store.save_task(&task).unwrap();
    task.record_decision("Unpublished native observation".into());
    task.charge(3.0);
    let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
    fresh.charge(2.0);
    fresh.record_decision("Concurrent helper observation".into());
    app.store.save_task(&fresh).unwrap();
    assert!(matches!(
        publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap(),
        AnswerHealing::Ready
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["It is red"]);
    assert_eq!(saved.cost_usd, 2.01);
    assert!(crate::verify::healing::reply::new_answers(&saved).is_empty());
    assert_eq!(task.criteria, saved.criteria);
    assert_eq!(task.cost_usd, 5.01);
    assert!(task
        .decisions
        .iter()
        .any(|line| line == "Unpublished native observation"));
    assert!(task
        .decisions
        .iter()
        .any(|line| line == "Concurrent helper observation"));
    assert!(CheckSnapshot::capture(&task).matches(&saved));
}

#[test]
fn a_new_owner_answer_discards_the_ruling_without_consuming_its_progress() {
    let (app, _dir) = test_app();
    let mut task = task();
    let captured = input(&task);
    let mut fresh = task.clone();
    fresh.record_decision("Owner: Which shade? -> crimson".into());
    app.store.save_task(&fresh).unwrap();
    assert!(matches!(
        publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap(),
        AnswerHealing::Retry
    ));
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["It is blue"]);
    assert_eq!(saved.brief_check.answers_seen, 0);
    assert_eq!(saved.cost_usd, 0.01);
    assert_eq!(crate::verify::healing::reply::new_answers(&saved).len(), 2);
    assert_eq!(task.cost_usd, 0.01);
    assert_eq!(task.brief_check.answers_seen, 0);
    assert_eq!(crate::verify::healing::reply::new_answers(&task).len(), 2);
}

#[test]
fn a_stopped_or_changed_owner_contract_is_never_overwritten_by_healing() {
    for stopped in [false, true] {
        let (app, _dir) = test_app();
        let mut task = task();
        let captured = input(&task);
        let mut fresh = task.clone();
        fresh.fixture_data().criteria = vec!["Owner supplied criterion".into()];
        if stopped {
            fresh.resume_phase(TaskStatus::Stopped, 42);
        }
        app.store.save_task(&fresh).unwrap();
        let result = publish(&app, &mut task, captured, ruling(), &CancelToken::new()).unwrap();
        assert!(matches!(result, AnswerHealing::Stopped));
        let saved = app.store.load_task(&task.id).unwrap().unwrap();
        assert_eq!(saved.criteria, fresh.criteria);
        assert_eq!(saved.status, fresh.status);
        assert_eq!(saved.brief_check.answers_seen, 0);
        assert_eq!(saved.cost_usd, 0.01);
    }
}

#[tokio::test]
async fn a_pending_owner_edit_is_retained_and_healing_yields() {
    let (app, _dir) = test_app();
    let mut task = task();
    let captured = input(&task);
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            criteria: Some(vec!["Owner changed the criterion".into()]),
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
    assert!(matches!(
        publish(&app, &mut task, captured, ruling(), &signals.cancel).unwrap(),
        AnswerHealing::Retry
    ));
    assert!(signals.pending_amend.lock().unwrap().is_some());
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.criteria, vec!["It is blue"]);
    assert_eq!(saved.brief_check.answers_seen, 0);
    assert_eq!(saved.cost_usd, 0.01);
}

#[tokio::test]
async fn a_pending_edit_after_healing_retry_preserves_the_current_best_of_receipt() {
    use crate::engine::test_support::attempt_with_failure;
    use crate::engine::verification::base::amendments::apply_pending_amendment;
    use crate::task::attempts::{record_run_completion, RunCompletion};
    use crate::task::Usage;
    let (app, _dir) = test_app();
    let mut task = task();
    task.fixture_data()
        .attempts
        .push(attempt_with_failure(1, "running"));
    app.store.save_task(&task).unwrap();
    let captured = input(&task);
    record_run_completion(
        &mut task,
        0,
        RunCompletion {
            session: Some("winner-b-session".into()),
            fingerprint: None,
            usage: Usage {
                input: 100,
                output: 20,
                cached: 30,
            },
            first_turn_tokens: None,
            cost: Some(0.4),
            estimated: false,
        },
    );
    task.fixture_data().attempts[0].route_id = "winner-b".into();
    task.fixture_data().attempts[0].summary = Some("B report".into());
    let signals = TaskSignals::new();
    *signals.pending_amend.lock().unwrap() = Some(PendingAmendment::capture(
        &task,
        Amendment {
            criteria: Some(vec!["Owner changed the criterion".into()]),
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
    let mut fresh = app.store.load_task(&task.id).unwrap().unwrap();
    fresh.record_decision("Owner: Which shade? -> crimson".into());
    app.store.save_task(&fresh).unwrap();
    assert!(matches!(
        publish(&app, &mut task, captured, ruling(), &signals.cancel).unwrap(),
        AnswerHealing::Retry
    ));
    assert_eq!(task.brief_check.answers_seen, 0);
    assert_eq!(crate::verify::healing::reply::new_answers(&task).len(), 2);
    assert!((task.cost_usd - 0.41).abs() < 0.000001);
    apply_pending_amendment(&app, &mut task, &signals.pending_amend, &signals.cancel).await;
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert!(signals.pending_amend.lock().unwrap().is_none());
    assert_eq!(saved.criteria, vec!["Owner changed the criterion"]);
    assert_eq!(saved.attempts.len(), 1);
    assert_eq!(saved.attempts[0].route_id, "winner-b");
    assert_eq!(
        saved.attempts[0].session_id.as_deref(),
        Some("winner-b-session")
    );
    assert_eq!(saved.attempts[0].summary.as_deref(), Some("B report"));
    let usage = saved.attempts[0].usage.as_ref().unwrap();
    assert_eq!((usage.input, usage.output, usage.cached), (100, 20, 30));
    assert!((saved.cost_usd - 0.41).abs() < 0.000001);
    assert_eq!(saved.brief_check.answers_seen, 0);
}
