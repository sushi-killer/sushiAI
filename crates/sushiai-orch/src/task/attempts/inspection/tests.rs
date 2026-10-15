use super::*;
use crate::task::attempts::{end, record_report, AttemptEnd, AttemptReport};
use crate::task::test_support::sample_task;
use crate::task::{FailureKind, TaskStatus};

fn blocked(task: &mut Task) -> BlockedInspection {
    record_report(
        task,
        0,
        AttemptReport {
            summary: Some("The requested API cannot exist".into()),
            handoff: Some("The evidence needs a decision".into()),
            declines: Vec::new(),
        },
    );
    end(
        task,
        0,
        AttemptEnd::Failed(Failure {
            kind: FailureKind::Blocked,
            detail: "An impossible criterion".into(),
            signature: "blocked:criterion".into(),
        }),
        42,
    );
    task.mark_attempt_blocked(0);
    BlockedInspection::capture(task, 0).unwrap()
}

#[test]
fn a_blocked_inspection_ends_only_its_attempt_and_retains_independent_paid_facts() {
    let mut stored = sample_task("blocked", TaskStatus::Running, AttemptStatus::Running);
    stored.fixture_data().attempts[0].cost_usd = Some(0.4);
    stored.fixture_data().cost_usd = 0.4;
    let mut local = stored.clone();
    let receipt = blocked(&mut local);
    stored.record_decision("Owner: retain the contract".into());
    receipt.apply(&mut stored).unwrap();
    receipt.apply(&mut stored).unwrap();
    assert_eq!(stored.status, TaskStatus::Running);
    assert_eq!(stored.attempts[0].status, AttemptStatus::Blocked);
    assert_eq!(stored.attempts[0].ended_at, Some(42));
    assert_eq!(stored.attempts[0].summary, local.attempts[0].summary);
    assert_eq!(stored.attempts[0].handoff, local.attempts[0].handoff);
    assert_eq!(
        stored.attempts[0].failure.as_ref().unwrap().kind,
        FailureKind::Blocked
    );
    assert_eq!(stored.cost_usd, 0.4);
    assert_eq!(stored.attempts[0].cost_usd, Some(0.4));
    assert_eq!(stored.decisions, ["Owner: retain the contract"]);
}

#[test]
fn a_replaced_attempt_cannot_receive_an_earlier_blocked_report() {
    let mut local = sample_task("blocked", TaskStatus::Running, AttemptStatus::Running);
    let mut replacement = local.clone();
    let receipt = blocked(&mut local);
    replacement.fixture_data().attempts[0].started_at += 1;
    let before = serde_json::to_value(&replacement).unwrap();
    assert!(receipt.apply(&mut replacement).is_err());
    assert_eq!(serde_json::to_value(&replacement).unwrap(), before);
}
