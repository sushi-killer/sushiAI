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

#[test]
fn a_carry_conflict_is_kept_in_resumed_and_fallback_briefs() {
    use crate::engine::attempts::resume::RunBrief;
    use crate::engine::lifecycle::implementation_brief::FullBrief;
    use crate::engine::lifecycle::selection::Selection;
    use crate::engine::test_support::attempt_with_failure;
    use crate::policy::routing::Route;
    use crate::policy::settings::Settings;
    use crate::task::attempt::FailureKind;
    let mut task =
        crate::engine::test_support::task_with_status(crate::task::state::TaskStatus::Running);
    let mut previous = attempt_with_failure(1, "conflict");
    previous.failure.as_mut().unwrap().kind = FailureKind::Conflict;
    previous.session_id = Some("existing-session".into());
    let route = Route {
        id: previous.route_id.clone(),
        harness: previous.harness,
        model: Some(previous.model.clone()),
        ..Settings::default().routes[0].clone()
    };
    task.fixture_data().attempts.push(previous);
    task.record_carry_conflicts(vec!["src/conflicted.rs".into()]);
    let selection = Selection {
        route,
        carry_conflicts: task.queue.carry_conflicts.clone(),
    };
    let brief = RunBrief::build(
        &mut task,
        &selection,
        FullBrief {
            text: "Full current contract\n\n## Report format\n".into(),
            messages: String::new(),
            advice: None,
        },
        false,
    );
    assert_eq!(brief.session.as_deref(), Some("existing-session"));
    for text in [&brief.current, &brief.full] {
        assert!(text.contains("## Base moved"));
        assert!(text.contains("src/conflicted.rs"));
        assert!(text.contains(&crate::delivery::git::carry::wip_ref(&task.id)));
    }
    assert!(brief.full.contains("Full current contract"));
}

#[test]
fn a_pr_check_repair_reaches_both_the_resumed_session_and_fresh_fallback() {
    use crate::engine::attempts::resume::RunBrief;
    use crate::engine::lifecycle::implementation_brief::FullBrief;
    use crate::engine::lifecycle::selection::Selection;
    use crate::engine::test_support::{attempt_with_failure, task_with_status};
    use crate::policy::routing::Route;
    use crate::policy::settings::Settings;
    use crate::task::TaskStatus;
    let mut task = task_with_status(TaskStatus::Running);
    let mut previous = attempt_with_failure(1, "verify");
    previous.session_id = Some("same-session".into());
    let route = Route {
        id: previous.route_id.clone(),
        harness: previous.harness,
        model: Some(previous.model.clone()),
        ..Settings::default().routes[0].clone()
    };
    task.fixture_data().attempts.push(previous);
    task.schedule_pr_check_repair(
        "exact".into(),
        crate::engine::decision::failure(
            FailureKind::Verify,
            "sh ci.sh: named_case_17 failed; exact output".into(),
        ),
    );
    let selection = Selection {
        route,
        carry_conflicts: vec![],
    };
    let brief = RunBrief::build(
        &mut task,
        &selection,
        FullBrief {
            text: "Current goal\n\n## Report format\n".into(),
            messages: String::new(),
            advice: None,
        },
        false,
    );
    assert_eq!(brief.session.as_deref(), Some("same-session"));
    for text in [&brief.current, &brief.full] {
        assert_eq!(text.matches("## PR check repair").count(), 1);
        assert!(
            text.contains("sh ci.sh")
                && text.contains("named_case_17")
                && text.contains("exact output")
        );
        assert!(text.contains("<untrusted-data>"));
        assert!(text.find("## PR check repair").unwrap() < text.find("## Report format").unwrap());
    }
}
