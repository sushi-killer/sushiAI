use super::{route, task};
use crate::policy::routing::Harness;
use crate::task::attempts::{self, AttemptEnd, RunObservation};
use crate::task::{AttemptStatus, Failure, FailureKind, Fingerprint, Stage, TaskStatus};

fn failure(kind: FailureKind) -> Failure {
    Failure {
        kind,
        detail: "synthetic failure".into(),
        signature: "already-classified".into(),
    }
}

#[test]
fn a_new_attempt_uses_the_next_number_and_inherits_no_previous_run_state() {
    let mut task = task();
    let route = route();
    let first = attempts::start(&mut task, Stage::Plan, &route, "drafting", 10);
    task.fixture_data().attempts[first].n = 7;
    attempts::observe_run(
        &mut task,
        7,
        RunObservation::Session("old-session".into()),
        11,
    );
    attempts::end(
        &mut task,
        first,
        AttemptEnd::Failed(failure(FailureKind::Error)),
        12,
    );

    let second = attempts::start(&mut task, Stage::Implement, &route, "implementation", 20);
    let attempt = &task.attempts[second];
    assert_eq!(attempt.n, 8);
    assert_eq!(attempt.stage, Stage::Implement);
    assert_eq!(attempt.route_id, "fixture-route");
    assert_eq!(attempt.harness, Harness::Claude);
    assert_eq!(attempt.model, "fixture-model");
    assert_eq!(attempt.reason, "implementation");
    assert_eq!(attempt.started_at, 20);
    assert_eq!(attempt.status, AttemptStatus::Running);
    assert!(attempt.ended_at.is_none() && attempt.failure.is_none());
    assert!(attempt.session_id.is_none() && attempt.fingerprint.is_none());
    assert!(attempt.cost_usd.is_none() && attempt.usage.is_none());
    assert!(attempt.verify.is_empty() && attempt.evidence.is_empty());
    assert_eq!(task.status, TaskStatus::Drafting);
    assert_eq!(task.updated_at, 20);
}

#[test]
fn the_plan_initializer_keeps_an_unspecified_model_empty() {
    let mut task = task();
    let route = crate::policy::routing::Route {
        model: None,
        ..route()
    };
    let index = attempts::start(&mut task, Stage::Plan, &route, "brief correction", 30);
    assert_eq!(task.attempts[index].stage, Stage::Plan);
    assert_eq!(task.attempts[index].model, "");
    assert_eq!(task.attempts[index].reason, "brief correction");
}

#[test]
fn observations_follow_the_attempt_number_and_first_edit_is_recorded_once() {
    let mut task = task();
    let route = route();
    attempts::start(&mut task, Stage::Implement, &route, "implementation", 10);
    attempts::start(&mut task, Stage::Plan, &route, "drafting", 20);
    task.fixture_data().attempts.swap(0, 1);
    let fingerprint = Fingerprint {
        models: vec!["observed-model".into()],
        harness: Harness::Claude,
        harness_version: Some("fixture-version".into()),
        prompt_hash: "fixture-prompt".into(),
    };
    for observation in [
        RunObservation::Session("native-session".into()),
        RunObservation::ProcessGroup(1234),
        RunObservation::Fingerprint(fingerprint.clone()),
        RunObservation::FirstEdit,
    ] {
        assert!(attempts::observe_run(&mut task, 1, observation, 30));
    }
    assert!(attempts::observe_run(
        &mut task,
        1,
        RunObservation::FirstEdit,
        40
    ));
    assert!(attempts::observe_run(
        &mut task,
        2,
        RunObservation::FirstEdit,
        50
    ));
    let attempt = &task.attempts[1];
    assert_eq!(attempt.session_id.as_deref(), Some("native-session"));
    assert_eq!(attempt.pgid, Some(1234));
    assert_eq!(attempt.fingerprint.as_ref(), Some(&fingerprint));
    assert_eq!(attempt.first_edit_at, Some(30));
    assert!(task.attempts[0].first_edit_at.is_none());
    assert!(!attempts::observe_run(
        &mut task,
        99,
        RunObservation::FirstEdit,
        60
    ));
    assert_eq!(task.updated_at, 60);
    assert_eq!(task.attempts.len(), 2);
}

#[test]
fn ending_an_attempt_keeps_the_task_phase_and_existing_diagnostics() {
    let mut task = task();
    let route = route();
    let index = attempts::start(&mut task, Stage::Implement, &route, "implementation", 10);
    task.fixture_data().attempts[index].summary = Some("work already described".into());
    attempts::end(
        &mut task,
        index,
        AttemptEnd::Failed(failure(FailureKind::Verify)),
        20,
    );
    assert_eq!(task.attempts[index].status, AttemptStatus::Failed);
    assert_eq!(task.attempts[index].ended_at, Some(20));
    assert_eq!(
        task.attempts[index].failure.as_ref().unwrap().signature,
        "already-classified"
    );
    assert_eq!(task.failed_count, 0);

    for (outcome, expected) in [
        (AttemptEnd::Interrupted, AttemptStatus::Interrupted),
        (AttemptEnd::Blocked, AttemptStatus::Blocked),
        (AttemptEnd::Passed, AttemptStatus::Passed),
    ] {
        attempts::end(&mut task, index, outcome, 30);
        assert_eq!(task.attempts[index].status, expected);
        assert_eq!(task.attempts[index].ended_at, Some(30));
        assert_eq!(
            task.attempts[index].summary.as_deref(),
            Some("work already described")
        );
        assert_eq!(
            task.attempts[index].failure.as_ref().unwrap().kind,
            FailureKind::Verify
        );
        assert_eq!(task.status, TaskStatus::Drafting);
    }
}

#[test]
fn a_spawn_that_never_started_spends_no_attempt() {
    let mut task = task();
    let route = route();
    attempts::start(&mut task, Stage::Plan, &route, "drafting", 10);
    let index = attempts::start(&mut task, Stage::Implement, &route, "implementation", 20);
    attempts::discard(&mut task, index, 30);
    assert_eq!(task.attempts.len(), 1);
    assert_eq!(attempts::implement_count(&task), 0);
    assert_eq!(attempts::next_number(&task), 2);
    assert_eq!(task.updated_at, 30);
}

#[test]
fn budget_counts_keep_interruptions_and_changeless_harness_errors_distinct() {
    let mut task = task();
    let route = route();
    attempts::start(&mut task, Stage::Plan, &route, "drafting", 10);
    let interrupted = attempts::start(&mut task, Stage::Implement, &route, "interrupted", 20);
    attempts::end(&mut task, interrupted, AttemptEnd::Interrupted, 21);
    let empty_error = attempts::start(&mut task, Stage::Implement, &route, "spawn failed", 30);
    attempts::end(
        &mut task,
        empty_error,
        AttemptEnd::Failed(failure(FailureKind::Error)),
        31,
    );
    let edited_error = attempts::start(&mut task, Stage::Implement, &route, "run failed", 40);
    task.fixture_data().attempts[edited_error].changed_files = vec!["changed.rs".into()];
    attempts::end(
        &mut task,
        edited_error,
        AttemptEnd::Failed(failure(FailureKind::Error)),
        41,
    );
    let red_check = attempts::start(&mut task, Stage::Implement, &route, "checks red", 50);
    attempts::end(
        &mut task,
        red_check,
        AttemptEnd::Failed(failure(FailureKind::Verify)),
        51,
    );
    assert_eq!(attempts::implement_count(&task), 4);
    assert_eq!(attempts::completed_implement_count(&task), 3);
    assert_eq!(attempts::counted_implement_count(&task), 2);
}
