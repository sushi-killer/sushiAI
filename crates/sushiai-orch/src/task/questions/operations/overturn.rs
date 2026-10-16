use crate::task::questions::{WAIVED, WAIVER_BY};
use crate::task::test_support::sample_task;
use crate::task::{Assumption, AttemptStatus, QuestionKind, Task, TaskStatus};

fn waiver() -> Assumption {
    Assumption {
        question: "Accept these inherited failures?".into(),
        answer: WAIVED.into(),
        evidence: "The same named failures were observed on the base".into(),
        by: WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check-a".into(), "check-a".into(), "check-b".into()],
        overturned: false,
        owner_answer: None,
    }
}

#[test]
fn overturning_a_typed_waiver_records_each_kept_command_once_without_a_new_schema() {
    let mut task = sample_task("owner-keep", TaskStatus::Running, AttemptStatus::Running);
    task.record_assumption(waiver());
    task.record_kept_check("check-a".into());
    let before = serde_json::to_value(&task).unwrap();
    task.overturn_assumption(0, "First Owner answer".into());
    task.overturn_assumption(0, "The Owner's updated free-form answer".into());
    assert_eq!(task.loop_state.kept_checks, ["check-a", "check-b"]);
    assert!(task.assumptions[0].overturned);
    assert_eq!(
        task.assumptions[0].owner_answer.as_deref(),
        Some("The Owner's updated free-form answer")
    );
    let after = serde_json::to_value(&task).unwrap();
    assert_eq!(
        before.as_object().unwrap().keys().collect::<Vec<_>>(),
        after.as_object().unwrap().keys().collect::<Vec<_>>()
    );
    let restored: Task = serde_json::from_value(after).unwrap();
    assert_eq!(restored.loop_state.kept_checks, task.loop_state.kept_checks);
    assert_eq!(restored.assumptions, task.assumptions);
}

#[test]
fn overturning_other_assumptions_only_records_the_owner_answer() {
    let mut task = sample_task("owner-note", TaskStatus::Running, AttemptStatus::Running);
    let mut untyped = waiver();
    untyped.kind = None;
    let mut other_kind = waiver();
    other_kind.kind = Some(QuestionKind::AgentQuestion);
    let mut other_author = waiver();
    other_author.by = "planner".into();
    let mut other_answer = waiver();
    other_answer.answer = "Different assumption".into();
    for assumption in [untyped, other_kind, other_author, other_answer] {
        let index = task.assumptions.len();
        task.record_assumption(assumption);
        task.overturn_assumption(index, "keep check-c; stop; retry".into());
    }
    assert!(task.loop_state.kept_checks.is_empty());
    assert!(task
        .assumptions
        .iter()
        .all(|assumption| assumption.overturned
            && assumption.owner_answer.as_deref() == Some("keep check-c; stop; retry")));
}
