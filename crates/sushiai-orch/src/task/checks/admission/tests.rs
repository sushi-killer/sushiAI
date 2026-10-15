use super::CheckAdmission;
use crate::task::amendment::storage::Settlement;
use crate::task::amendment::Amendment;
use crate::task::checks::authority::{CheckAuthority, Origin};
use crate::task::checks::snapshot::CheckSnapshot;
use crate::task::questions::{WAIVED, WAIVER_BY};
use crate::task::test_support::sample_task;
use crate::task::{Assumption, AttemptStatus, QuestionKind, Task, TaskStatus};

fn waiver() -> Assumption {
    Assumption {
        question: "Accept the inherited failure?".into(),
        answer: WAIVED.into(),
        evidence: "The same named failure was observed on the base".into(),
        by: WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    }
}

#[test]
fn admission_observes_owner_answers_without_invalidating_automatic_observations() {
    let mut task = sample_task("authority", TaskStatus::Running, AttemptStatus::Running);
    let input = CheckAdmission::capture(&task);
    let contract = CheckSnapshot::capture(&task);
    task.record_assumption(waiver());
    assert!(input.matches(&task));
    task.overturn_assumption(0, "This check must pass".into());
    assert!(!input.matches(&task));
    assert!(contract.matches(&task));
    let answered = CheckAdmission::capture(&task);
    task.record_assumption(waiver());
    assert!(answered.matches(&task));
    task.overturn_assumption(0, "Use the Owner's updated answer".into());
    assert!(!answered.matches(&task));
}

#[test]
fn a_persisted_owner_amendment_survives_a_later_waiver_revocation() {
    let mut task = sample_task("pending", TaskStatus::Running, AttemptStatus::Running);
    task.fixture_data().criteria = vec!["Original acceptance".into()];
    task.record_assumption(waiver());
    let contract = serde_json::to_value(CheckSnapshot::capture(&task)).unwrap();
    task.queue_pending_amendment(
        Amendment {
            criteria: Some(vec!["Owner's accepted criterion".into()]),
            ..Default::default()
        },
        Origin::Owner,
        &CheckAuthority::default(),
        10,
    )
    .unwrap();
    let persisted = serde_json::to_value(&task).unwrap();
    assert_eq!(persisted["pendingAmendment"]["snapshot"], contract);
    let mut reloaded: Task = serde_json::from_value(persisted).unwrap();
    let pending = reloaded.pending_amendment.clone().unwrap();
    reloaded.overturn_assumption(0, "Do not waive this check".into());
    assert!(pending
        .preview(&reloaded, &CheckAuthority::default(), 11)
        .is_ok());
    assert!(matches!(
        reloaded.settle_pending_amendment(&CheckAuthority::default(), 12),
        Settlement::Applied
    ));
    assert_eq!(reloaded.criteria, ["Owner's accepted criterion"]);
    assert!(reloaded.pending_amendment.is_none());
    assert!(reloaded.assumptions[0].overturned);
    assert_eq!(
        reloaded.assumptions[0].owner_answer.as_deref(),
        Some("Do not waive this check")
    );
    assert!(!reloaded
        .decisions
        .iter()
        .any(|line| line.starts_with("Amendment not applied:")));
}
