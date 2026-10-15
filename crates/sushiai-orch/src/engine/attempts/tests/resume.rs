use crate::task::attempt::{FailureKind, VerifyOutcome};
use crate::task::checks::types::{Baseline, Check};

use super::build_resume_brief;
use crate::engine::attempts::test_fixtures::{failed_implement_attempt, sample_task};

#[test]
fn a_resumed_brief_replaces_the_old_plan_and_labels_old_commands_as_history() {
    let mut task = sample_task();
    task.fixture_data().criteria = vec!["The revised behavior works".into()];
    task.fixture_data().verify = vec!["check-current".into()];
    task.fixture_data().final_verify = vec!["check-final-current".into()];
    task.fixture_data().checks = vec![Check {
        criterion: 0,
        run: "check-criterion-current".into(),
        baseline: Some(Baseline::Fail),
    }];
    let mut previous = failed_implement_attempt("check-removed exited 1".into());
    previous.verify = vec![VerifyOutcome {
        command: "check-removed".into(),
        code: Some(1),
        tail: "old failure".into(),
        ..Default::default()
    }];
    let brief = build_resume_brief(&task, &previous, "", None);
    let (current, history) = brief
        .split_once("## Previous attempt evidence (history)")
        .unwrap();
    assert!(current.contains("The revised behavior works"), "{brief}");
    for command in [
        "check-current",
        "check-final-current",
        "check-criterion-current",
    ] {
        assert!(current.contains(command), "{brief}");
    }
    assert!(!current.contains("check-removed"), "{brief}");
    assert!(history.contains("check-removed"), "{brief}");
}

#[test]
fn a_time_budget_resume_asks_for_continued_work_without_inventing_failed_checks() {
    let task = sample_task();
    let mut previous = failed_implement_attempt("Wall-clock budget reached".into());
    previous.failure.as_mut().unwrap().kind = FailureKind::TimeBudget;
    previous.handoff = Some("The reader is done; finish the writer".into());
    let brief = build_resume_brief(&task, &previous, "", None);
    assert!(
        brief.starts_with("## Continue after the time budget"),
        "{brief}"
    );
    assert!(
        brief.contains("The reader is done; finish the writer"),
        "{brief}"
    );
    assert!(!brief.contains("## Failed checks"), "{brief}");
    assert!(!brief.contains("fix the failed checks"), "{brief}");
}
