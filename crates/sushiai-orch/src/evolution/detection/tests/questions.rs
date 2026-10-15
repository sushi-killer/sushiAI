use super::*;

#[test]
fn evolution_owner_question_one_per_owner_decision() {
    let mut t = task("t", 10);
    t.fixture_data().question_history = vec![
        crate::engine::evolution::test_support::owner_answer("which db?", "sqlite"),
        crate::engine::evolution::test_support::owner_answer("dependency?", "answered"),
    ];
    t.fixture_data().decisions = vec!["Planner: tier hard".into()];
    let s = run(&t, &[], &[]);
    let o = of_kind(&s, SignalKind::OwnerQuestion);
    assert_eq!(o.len(), 2);
    assert_eq!(o[0].attempt, 0);
    assert_eq!(o[0].excerpt_ref.file, "tasks/t/task.json");
    assert_eq!(o[0].wasted_calls, 0);
}

#[test]
fn evolution_counts_owner_questions_without_lead_answers() {
    let mut task = task_with_status(TaskStatus::Done);
    let mut lead = crate::engine::evolution::test_support::owner_answer("q", "a");
    lead.answered_by = AnsweredBy::Lead;
    task.fixture_data().question_history = vec![
        crate::engine::evolution::test_support::owner_answer("A", "B"),
        lead,
    ];
    let signals = Detection::for_task(&task, &AttemptEvents::new(), &Settings::default()).finish(
        std::slice::from_ref(&task),
        &[],
        &[],
    );
    assert_eq!(
        signals
            .iter()
            .filter(|s| s.kind == SignalKind::OwnerQuestion)
            .count(),
        1
    );
}
