use super::*;
use crate::task::{DeclineReason, FindingScenario, TaskStatus};

fn review(findings: &[&str]) -> ReviewResult {
    ReviewResult {
        verdict: Verdict::Fail,
        findings: findings.iter().map(|s| s.to_string()).collect(),
        repeated: Vec::new(),
        severities: findings.iter().map(|_| Some(1)).collect(),
        details: Vec::new(),
        criteria: Vec::new(),
    }
}

fn named(r: &mut ReviewResult, ids: &[Option<&str>]) {
    r.details = ids
        .iter()
        .map(|id| FindingDetail {
            id: id.map(str::to_string),
            ..Default::default()
        })
        .collect();
}

#[test]
fn a_mark_is_added_stripped_and_read_back() {
    let marked = with_id("P1: the parser drops the last row", "F-7");
    assert_eq!(marked, "P1: the parser drops the last row [F-7]");
    assert_eq!(
        strip_finding_id(&marked),
        "P1: the parser drops the last row"
    );
    assert_eq!(
        with_id(&marked, "F-8"),
        "P1: the parser drops the last row [F-8]"
    );
}

#[test]
fn a_registry_entry_flips_to_resolved_when_a_review_omits_it() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut r = review(&["P1: first problem here"]);
    register_findings(&mut task, 1, 1, &mut r);
    assert_eq!(task.findings.len(), 1);
    assert_eq!(task.findings[0].status, FindingStatus::Open);
    assert!(r.findings[0].ends_with(&format!("[{}]", task.findings[0].id)));
    let mut next = review(&["P1: other problem entirely"]);
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Resolved);
    assert_eq!(task.findings.len(), 2);
}

#[test]
fn a_new_p1_worded_like_a_resolved_one_with_no_id_is_kept() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut r = review(&["P1: the parser drops the last row"]);
    register_findings(&mut task, 1, 1, &mut r);
    let mut next = review(&["P2: something else"]);
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Resolved);

    let mut again = review(&["P1: the parser drops the last row"]);
    named(&mut again, &[None]);
    register_findings(&mut task, 3, 1, &mut again);
    assert_eq!(again.findings.len(), 1, "{:?}", again.findings);
    assert_eq!(task.findings.len(), 3);
    assert_eq!(again.details[0].id.as_deref(), Some("F-3"));
    assert!(again.repeated.is_empty());
}

#[test]
fn a_named_resolved_finding_is_never_dropped_by_its_id_alone() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut r = review(&["P1: first"]);
    register_findings(&mut task, 1, 1, &mut r);
    let mut next = review(&["P1: second"]);
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Resolved);

    let mut back = review(&["P1: first, reworded"]);
    named(&mut back, &[Some("F-1")]);
    register_findings(&mut task, 3, 1, &mut back);
    assert_eq!(back.findings, ["P1: first, reworded [F-1]"]);
    assert_eq!(back.repeated, back.findings);
    assert_eq!(back.verdict, Verdict::Fail);
    assert_eq!(task.findings[0].status, FindingStatus::Open);
    assert_eq!(task.findings[0].repeats, HANDLE_AT_REPEATS);

    let mut next = review(&["P1: second"]);
    named(&mut next, &[Some("F-2")]);
    register_findings(&mut task, 4, 1, &mut next);
    let mut with_run = review(&["P1: first, reworded"]);
    with_run.details = vec![FindingDetail {
        id: Some("F-1".into()),
        scenario: Some(FindingScenario {
            run: "cargo test first".into(),
            fails: "red".into(),
        }),
        ..Default::default()
    }];
    register_findings(&mut task, 5, 1, &mut with_run);
    assert_eq!(task.findings[0].scenarios, ["cargo test first"]);
    assert_eq!(with_run.findings.len(), 1);
}

#[test]
fn a_named_open_finding_is_a_repeat_and_counts_once_per_attempt() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut r = review(&["P1: first"]);
    register_findings(&mut task, 1, 1, &mut r);
    assert!(r.repeated.is_empty());
    for (attempt, round, repeats, repeated) in [
        (1, 1, 0, false),
        (1, 2, 0, true),
        (2, 1, 1, true),
        (2, 2, 1, true),
    ] {
        let mut again = review(&["P1: first, reworded"]);
        named(&mut again, &[Some("F-1")]);
        register_findings(&mut task, attempt, round, &mut again);
        assert_eq!(task.findings.len(), 1);
        assert_eq!(task.findings[0].repeats, repeats);
        assert_eq!(again.repeated.len(), usize::from(repeated));
    }
}

fn decline(id: &str) -> Decline {
    Decline {
        id: id.into(),
        reason: DeclineReason::Wrong,
        evidence: vec!["src/parse.rs:9 keeps the row".into()],
    }
}

fn ruling(id: &str, upheld: bool) -> DeclineRuling {
    DeclineRuling {
        id: id.into(),
        upheld,
    }
}

#[test]
fn an_upheld_decline_resolves_its_finding_with_no_judge_turn() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut first = review(&["P1: the parser drops the last row"]);
    register_findings(&mut task, 1, 1, &mut first);
    let id = task.findings[0].id.clone();
    let mut next = review(&["P1: other problem entirely"]);
    apply_declines(
        &mut task,
        2,
        &mut next,
        &[decline(&id)],
        &[ruling(&id, true)],
    );
    assert_eq!(task.findings[0].status, FindingStatus::Resolved);
    assert_eq!(next.findings, ["P1: other problem entirely"]);
    assert!(task.decisions.iter().any(|d| d.contains("upheld")));
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Resolved);
    assert_eq!(task.findings.len(), 2);
}

#[test]
fn an_upheld_decline_drops_the_reviewers_copy_of_its_finding() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut first = review(&["P1: the parser drops the last row"]);
    register_findings(&mut task, 1, 1, &mut first);
    let id = task.findings[0].id.clone();
    let mut next = review(&["P1: the parser drops the last row"]);
    named(&mut next, &[Some(id.as_str())]);
    apply_declines(
        &mut task,
        2,
        &mut next,
        &[decline(&id)],
        &[ruling(&id, true)],
    );
    assert!(next.findings.is_empty(), "{:?}", next.findings);
    assert_eq!(next.verdict, Verdict::Pass);
}

#[test]
fn a_declined_finding_the_review_does_not_uphold_stays_blocking() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut first = review(&["P1: the parser drops the last row"]);
    register_findings(&mut task, 1, 1, &mut first);
    let id = task.findings[0].id.clone();
    let mut next = review(&["P1: other problem entirely"]);
    apply_declines(
        &mut task,
        2,
        &mut next,
        &[decline(&id)],
        &[ruling(&id, false)],
    );
    assert_eq!(next.verdict, Verdict::Fail);
    assert!(
        next.findings
            .iter()
            .any(|f| f == &format!("P1: the parser drops the last row [{id}]")),
        "{:?}",
        next.findings
    );
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Open);
    assert_eq!(next.details[1].id.as_deref(), Some(id.as_str()));
}

#[test]
fn a_decline_pairs_only_by_the_id_it_names() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut first = review(&[
        "P1: the parser drops the last row",
        "P1: the cache is stale",
    ]);
    register_findings(&mut task, 1, 1, &mut first);
    let (f1, f2) = (task.findings[0].id.clone(), task.findings[1].id.clone());
    // F-1 is the open finding; the decline names no registered id, so
    // it is never paired with F-1 by position or by wording.
    let mut next = review(&[
        "P1: the parser drops the last row",
        "P1: the cache is stale",
    ]);
    named(&mut next, &[Some(f1.as_str()), Some(f2.as_str())]);
    apply_declines(
        &mut task,
        2,
        &mut next,
        &[decline("F-99")],
        &[ruling("F-99", true)],
    );
    assert_eq!(task.findings[0].status, FindingStatus::Open);
    assert_eq!(task.findings[1].status, FindingStatus::Open);
    assert_eq!(next.findings.len(), 2, "{:?}", next.findings);
    assert_eq!(next.verdict, Verdict::Fail);
    assert!(task.assumptions.is_empty());
}

#[test]
fn a_decline_with_no_ruling_is_blocking_too() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut first = review(&["P1: the parser drops the last row"]);
    register_findings(&mut task, 1, 1, &mut first);
    let id = task.findings[0].id.clone();
    let mut next = review(&["P1: other problem entirely"]);
    apply_declines(&mut task, 2, &mut next, &[decline(&id)], &[]);
    assert_eq!(next.verdict, Verdict::Fail);
    register_findings(&mut task, 2, 1, &mut next);
    assert_eq!(task.findings[0].status, FindingStatus::Open);
}

#[test]
fn a_p0_and_a_p3_with_the_same_wording_stay_distinct() {
    let mut task = crate::task::test_support::sample_task(
        "finding",
        TaskStatus::Running,
        crate::task::AttemptStatus::Failed,
    );
    let mut r = review(&["P0: the same words", "P3: the same words"]);
    register_findings(&mut task, 1, 1, &mut r);
    assert_eq!(task.findings.len(), 2);
    assert_ne!(task.findings[0].id, task.findings[1].id);
}
