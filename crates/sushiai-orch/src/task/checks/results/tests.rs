use super::*;
use crate::task::test_support::sample_task;
use crate::task::{Check, CriterionRuling, ReviewResult, TaskStatus, Verdict, VerifyOutcome};

fn task_and_attempt() -> (Task, Attempt) {
    let mut task = sample_task(
        "criterion",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    task.fixture_data().criteria = vec![
        "Alpha works -- check: read it".into(),
        "Beta works".into(),
        "Gamma works".into(),
        "Delta works".into(),
    ];
    let attempt = task.attempts[0].clone();
    (task, attempt)
}

fn outcome(command: &str, code: i32) -> VerifyOutcome {
    VerifyOutcome {
        command: command.into(),
        code: Some(code),
        tail: String::new(),
        ms: 1,
        ..Default::default()
    }
}

fn ruling(index: Option<usize>, met: Option<bool>) -> CriterionRuling {
    CriterionRuling {
        criterion: format!("criterion {index:?}"),
        index,
        met,
        evidence: Some(format!("because {index:?}")),
    }
}

fn statuses(task: &Task, attempt: &Attempt) -> Vec<CriterionStatus> {
    criteria_results(task, attempt)
        .into_iter()
        .map(|r| r.status)
        .collect()
}

#[test]
fn everything_is_pending_before_a_check_or_review_ran() {
    let (task, attempt) = task_and_attempt();
    let results = criteria_results(&task, &attempt);
    assert_eq!(results.len(), 4);
    assert!(results.iter().all(|r| r.status == CriterionStatus::Pending));
    assert_eq!(results[1].text, "Beta works");
    assert_eq!(results[1].criterion, 1);
}

#[test]
fn checks_and_rulings_merge_with_failing_over_met_over_pending() {
    use CriterionStatus::*;
    let (mut task, mut attempt) = task_and_attempt();
    task.fixture_data().checks = vec![
        Check {
            criterion: 0,
            run: "t0".into(),
            baseline: None,
        },
        Check {
            criterion: 1,
            run: "t1".into(),
            baseline: None,
        },
        Check {
            criterion: 2,
            run: "t2".into(),
            baseline: None,
        },
    ];
    task.fixture_data().held_out = Some(Check {
        criterion: 3,
        run: "secret".into(),
        baseline: None,
    });
    attempt.verify = vec![
        outcome("t0", 0),
        outcome("t1", 1),
        outcome("t2", 0),
        outcome(&crate::task::checks::commands::held_out_label(3), 0),
    ];
    assert_eq!(statuses(&task, &attempt), vec![Met, Failing, Met, Met]);
    // A passing check does not outrank a `met: false` ruling, and a
    // failing check is not rescued by `met: true`.
    attempt.review = Some(ReviewResult {
        verdict: Verdict::Fail,
        findings: vec![],
        repeated: vec![],
        severities: vec![],
        details: Vec::new(),
        criteria: vec![
            ruling(Some(0), Some(false)),
            ruling(Some(1), Some(true)),
            ruling(Some(2), None),
        ],
    });
    let results = criteria_results(&task, &attempt);
    let got: Vec<_> = results.iter().map(|r| r.status).collect();
    assert_eq!(got, vec![Failing, Failing, Met, Met]);
    assert_eq!(results[0].evidence.as_deref(), Some("because Some(0)"));
    assert!(!results[3].evidence.clone().unwrap().contains("secret"));
}

#[test]
fn rulings_alone_decide_and_a_null_ruling_stays_pending() {
    use CriterionStatus::*;
    let (task, mut attempt) = task_and_attempt();
    attempt.review = Some(ReviewResult {
        verdict: Verdict::Pass,
        findings: vec![],
        repeated: vec![],
        severities: vec![],
        details: Vec::new(),
        criteria: vec![
            ruling(Some(0), Some(true)),
            ruling(Some(1), None),
            ruling(Some(2), Some(false)),
            ruling(None, Some(false)),
            ruling(Some(9), Some(false)),
        ],
    });
    assert_eq!(
        statuses(&task, &attempt),
        vec![Met, Pending, Failing, Pending]
    );
}

#[test]
fn a_ruling_for_the_longer_of_two_prefixed_criteria_does_not_settle_the_shorter() {
    use CriterionStatus::*;
    let (mut task, mut attempt) = task_and_attempt();
    task.fixture_data().criteria = vec!["Beta".into(), "Beta works -- check: read it".into()];
    attempt.review = Some(ReviewResult {
        verdict: Verdict::Pass,
        findings: vec![],
        repeated: vec![],
        severities: vec![],
        details: Vec::new(),
        criteria: vec![ruling(Some(1), Some(true))],
    });
    assert_eq!(statuses(&task, &attempt), vec![Pending, Met]);
}
