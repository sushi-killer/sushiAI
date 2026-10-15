use crate::engine::attempts::brief::build_brief;
use crate::prompts::brief_text;
use crate::task::attempt::{FailureKind, VerifyOutcome};
use crate::task::checks::commands::held_out_label;
use crate::task::review::{Decline, DeclineReason, FindingRecord, FindingStatus};
use crate::verify::review::routing::Lenses;

use super::{build_narrow_review_brief, build_review_brief, review_retry_note, NarrowReview};
use crate::engine::attempts::test_fixtures::{
    failed_implement_attempt, grounded_task, sample_task,
};

#[test]
fn the_review_brief_numbers_the_criteria_and_carries_the_findings_registry() {
    let mut task = sample_task();
    task.fixture_data().criteria = vec!["Alpha".into(), "Alpha and beta".into()];
    let one = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        "d",
        &Lenses::ALL,
    );
    assert!(!one.contains("## Previous review"), "{one}");
    assert!(one.contains("\n0. Alpha\n1. Alpha and beta\n"), "{one}");
    assert!(one.contains("\"causedByDiff\":true"), "{one}");
    assert!(!one.contains("\"repeat\""), "{one}");
    task.fixture_data().findings = vec![
        FindingRecord {
            id: "F-1".into(),
            text: "P1: a.rs:1 - null check missing".into(),
            status: FindingStatus::Open,
            attempt: 1,
            round: 1,
            repeats: 0,
            scenarios: Vec::new(),
            handled: false,
        },
        FindingRecord {
            id: "F-2".into(),
            text: "P2: b.rs:2 - old".into(),
            status: FindingStatus::Resolved,
            attempt: 1,
            round: 1,
            repeats: 0,
            scenarios: Vec::new(),
            handled: false,
        },
    ];
    let two = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        "d",
        &Lenses::ALL,
    );
    assert!(two.contains("## Previous review"), "{two}");
    assert!(
        two.contains(
            "F-1 (open): P1: a.rs:1 - null check missing\nF-2 (resolved): P2: b.rs:2 - old"
        ),
        "{two}"
    );
}

#[test]
fn the_review_brief_states_the_rule_and_carries_agent_lines() {
    let task = sample_task();
    let text = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "Implementer report: complete",
            agent_decisions: &["Agent: kept the old name".into()],
            verify_results: &[],
            screenshots: &[("artifacts/a.png".into(), Some(1))],
            evidence_from: None,
            declines: &[],
        },
        "diff body",
        &Lenses::ALL,
    );
    assert!(text.starts_with("## Review"));
    assert!(text.contains("FAIL only for an unmet acceptance criterion or a P0/P1 finding"));
    assert!(text.contains("\"P2: path:line - issue\""));
    let example = brief_text("review_report_example");
    let value: serde_json::Value = serde_json::from_str(example).unwrap();
    assert!(value["criteria"].is_array() && value["findings"].is_array());
    assert!(text.contains(example));
    assert!(review_retry_note().contains(example));
    assert!(text.contains("<untrusted-data>\nAgent: kept the old name\n</untrusted-data>"));
    assert!(text.contains("judged on its merits"));
    assert!(text.contains("- `artifacts/a.png`"));
    assert!(text.contains("Rule on every acceptance criterion"));
}

#[test]
fn the_review_brief_lists_ungrounded_checks_and_the_held_out_result_by_criterion() {
    let task = grounded_task();
    let ran = |code| VerifyOutcome {
        command: held_out_label(1),
        code,
        tail: String::new(),
        ms: 1,
        ..Default::default()
    };
    let failed = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[ran(Some(1))],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        "",
        &Lenses::ALL,
    );
    assert!(failed.contains("`vacuous-cmd` already passed on the base, so it is not grounded"));
    assert!(failed.contains("Held-out check for criterion 1 (Click saves settings): failed."));
    assert!(!failed.contains("SECRET-held-out-cmd"));
    let passed = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[ran(Some(0))],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        "",
        &Lenses::ALL,
    );
    assert!(passed.contains("Held-out check for criterion 1 (Click saves settings): passed."));
    // The held-out outcome is not also listed as a verify result.
    assert!(!passed.contains("-> exit"));
}

#[test]
fn the_review_brief_keeps_a_held_out_named_result_without_a_variant() {
    let mut task = grounded_task();
    task.fixture_data().variant = None;
    for (code, outcome) in [(Some(0), "passed"), (Some(1), "failed")] {
        let ran = VerifyOutcome {
            command: held_out_label(1),
            code,
            tail: String::new(),
            ms: 1,
            ..Default::default()
        };
        let text = build_review_brief(
            &task,
            &crate::verify::review::brief::ReviewEvidence {
                implementer_note: "note",
                agent_decisions: &[],
                verify_results: &[ran],
                screenshots: &[],
                evidence_from: None,
                declines: &[],
            },
            "",
            &Lenses::ALL,
        );
        let result = format!("Held-out check for criterion 1 (Click saves settings): {outcome}.");
        assert_eq!(text.matches(&result).count(), 1, "{text}");
        assert!(!text.contains("SECRET-held-out-cmd"));
        assert!(!text.contains("-> exit"));
    }
}

#[test]
fn the_declines_field_is_explained_only_after_a_review_failure() {
    let mut task = sample_task();
    assert!(!build_brief(&task, "", "").contains("declines"));
    task.fixture_data()
        .attempts
        .push(failed_implement_attempt("x".into()));
    assert!(!build_brief(&task, "", "").contains("declines"));
    task.fixture_data().attempts[0]
        .failure
        .as_mut()
        .unwrap()
        .kind = crate::task::attempt::FailureKind::Review;
    let brief = build_brief(&task, "", "");
    assert!(brief.contains("\"declines\""));
    assert!(brief.contains("decline it with evidence instead of working around it"));
    assert!(!brief.contains("judge"));
    // Only the latest implement attempt counts.
    let mut second = task.attempts[0].clone();
    second.n = 2;
    second.failure.as_mut().unwrap().kind = FailureKind::Verify;
    task.fixture_data().attempts.push(second);
    assert!(!build_brief(&task, "", "").contains("declines"));
}

#[test]
fn a_narrow_re_review_is_briefed_with_the_delta_and_the_prior_report_only() {
    let mut task = sample_task();
    task.fixture_data()
        .findings
        .push(crate::task::review::FindingRecord {
            id: "F-1".into(),
            text: "P1: a.rs:1 - null check missing".into(),
            status: FindingStatus::Open,
            attempt: 1,
            round: 1,
            repeats: 0,
            scenarios: Vec::new(),
            handled: false,
        });
    let narrow = NarrowReview {
        open_criteria: &[],
        settled_criteria: &[],
        delta: "+let checked = true;",
    };
    let text = build_narrow_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        &narrow,
    );
    assert!(text.contains("+let checked = true;"));
    assert!(text.contains("## Previous review"));
    assert!(text.contains("P1: a.rs:1 - null check missing"));
    assert!(!text.contains("## Diff"));
    assert!(!text.contains("## Review areas"));
}

#[test]
fn declined_findings_get_a_review_section_the_reviewer_rules_on() {
    let task = sample_task();
    let declines = [Decline {
        id: "F-1".into(),
        reason: DeclineReason::Wrong,
        evidence: vec!["a.rs:4 keeps the row".into()],
    }];
    let text = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &declines,
        },
        "d",
        &Lenses::ALL,
    );
    assert!(text.contains("## Declined findings"));
    assert!(text.contains("F-1 (wrong): a.rs:4 keeps the row"));
    assert!(text.contains("\"declineRulings\""));
    let none = build_review_brief(
        &task,
        &crate::verify::review::brief::ReviewEvidence {
            implementer_note: "note",
            agent_decisions: &[],
            verify_results: &[],
            screenshots: &[],
            evidence_from: None,
            declines: &[],
        },
        "d",
        &Lenses::ALL,
    );
    assert!(!none.contains("## Declined findings"));
}
