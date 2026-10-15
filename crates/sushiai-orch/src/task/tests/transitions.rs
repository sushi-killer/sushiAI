use super::{route, task};

use crate::task::attempts::{self, AttemptEnd, RunObservation};
use crate::task::transitions::{self, Completion, PauseEnd};
use crate::task::{
    AnswerSource, AttemptStatus, Failure, FailureKind, ParkRecord, Stage, TaskStatus,
};
use serde_json::json;

#[test]
fn reopening_retains_the_native_session_and_report_while_clearing_the_failed_end() {
    let mut task = task();
    let index = attempts::start(&mut task, Stage::Implement, &route(), "implementation", 10);
    attempts::observe_run(
        &mut task,
        1,
        RunObservation::Session("native-session".into()),
        11,
    );
    task.fixture_data().attempts[index].summary = Some("partial result".into());
    attempts::end(
        &mut task,
        index,
        AttemptEnd::Failed(Failure {
            kind: FailureKind::Verify,
            detail: "synthetic red check".into(),
            signature: "fixture-check".into(),
        }),
        20,
    );
    transitions::reopen(&mut task, index, 30);
    assert_eq!(task.status, TaskStatus::Running);
    assert_eq!(task.attempts[index].status, AttemptStatus::Running);
    assert!(task.attempts[index].ended_at.is_none());
    assert!(task.attempts[index].failure.is_none());
    assert_eq!(
        task.attempts[index].session_id.as_deref(),
        Some("native-session")
    );
    assert_eq!(
        task.attempts[index].summary.as_deref(),
        Some("partial result")
    );
    assert_eq!(task.updated_at, 30);
}

#[test]
fn completing_a_task_passes_its_attempt_or_finishes_a_parent_without_one() {
    for (outcome, expected) in [
        (Completion::Done, TaskStatus::Done),
        (Completion::WaitingForLanding, TaskStatus::Landing),
    ] {
        let mut task = task();
        let index = attempts::start(&mut task, Stage::Implement, &route(), "implementation", 10);
        transitions::complete(&mut task, Some(index), outcome, 20);
        assert_eq!(task.status, expected);
        assert_eq!(task.attempts[index].status, AttemptStatus::Passed);
        assert_eq!(task.attempts[index].ended_at, Some(20));
        assert_eq!(task.failed_count, 0);
        assert_eq!(task.updated_at, 20);
    }
    let mut parent = task();
    transitions::complete(&mut parent, None, Completion::Done, 40);
    assert_eq!(parent.status, TaskStatus::Done);
    assert!(parent.attempts.is_empty());
}

#[test]
fn failure_notices_count_each_explicit_failure_transition() {
    let mut task = task();
    transitions::fail(&mut task, 10);
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.failed_count, 1);
    transitions::fail(&mut task, 20);
    assert_eq!(task.failed_count, 2);
    assert_eq!(task.updated_at, 20);
}

#[test]
fn a_limit_pause_is_interrupted_and_preserved_only_across_shutdown() {
    let mut task = task();
    let index = attempts::start(&mut task, Stage::Implement, &route(), "implementation", 10);
    transitions::pause_for_limit(&mut task, index, 1_000, 20);
    assert_eq!(task.status, TaskStatus::Queued);
    assert_eq!(task.attempts[index].status, AttemptStatus::Interrupted);
    assert_eq!(task.attempts[index].ended_at, Some(20));
    assert!(task.attempts[index].failure.is_none());
    assert_eq!(task.queue.rate_pauses, 1);
    assert_eq!(task.queue.paused_attempts, 1);
    assert_eq!(task.queue.paused_until, Some(1_000));
    assert_eq!(attempts::counted_implement_count(&task), 0);

    for (outcome, expected) in [
        (
            PauseEnd::Cancelled {
                shutting_down: true,
            },
            Some(1_000),
        ),
        (
            PauseEnd::Cancelled {
                shutting_down: false,
            },
            None,
        ),
        (PauseEnd::Elapsed, None),
    ] {
        let mut paused = task.clone();
        transitions::end_limit_pause(&mut paused, outcome, 30);
        assert_eq!(paused.queue.paused_until, expected);
        assert_eq!(paused.queue.paused_attempts, 1);
        assert_eq!(paused.updated_at, 30);
    }
}

#[test]
fn recovery_keeps_saved_stops_parks_and_terminal_statuses() {
    let mut stopped = task();
    stopped.fixture_data().queue.stopped_explicitly = true;
    stopped.fixture_data().question =
        Some(serde_json::from_value(json!({"text":"old question","options":[]})).unwrap());
    transitions::recover_interruption(&mut stopped, 20);
    assert_eq!(stopped.status, TaskStatus::Stopped);
    assert!(stopped.question.is_none());

    let mut parked = task();
    parked.fixture_data().queue.park = Some(ParkRecord {
        reason: "owner parked the task".into(),
        until: None,
        by: AnswerSource::Owner,
    });
    transitions::recover_interruption(&mut parked, 30);
    assert_eq!(parked.status, TaskStatus::Queued);
    assert!(parked.queue.park.is_some());

    for status in [
        TaskStatus::Stopped,
        TaskStatus::Failed,
        TaskStatus::Done,
        TaskStatus::Running,
    ] {
        let mut interrupted = task();
        interrupted.fixture_data().status = status;
        transitions::recover_interruption(&mut interrupted, 40);
        assert_eq!(
            interrupted.status,
            if status == TaskStatus::Running {
                TaskStatus::Queued
            } else {
                status
            }
        );
        assert_eq!(interrupted.failed_count, 0);
        assert_eq!(interrupted.updated_at, 40);
    }
}
