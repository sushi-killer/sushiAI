use crate::policy::routing::Harness;
use crate::task::attempt::{Attempt, AttemptStatus, Stage};
use crate::task::review::Verdict;
use crate::verify::review::reply::parse_review;

use super::build_brief;
use crate::engine::attempts::test_fixtures::{
    dependent_of, failed_implement_attempt, grounded_task, sample_task,
};

#[test]
fn brief_is_identical_regardless_of_which_harness_will_run_it() {
    // build_brief takes no harness parameter at all: the same task and
    // git state always produce the same text, whether the caller is
    // about to hand it to Claude or to Codex.
    let task = sample_task();
    let for_claude = build_brief(&task, " M src/x.ts\n", " 1 file changed\n");
    let for_codex = build_brief(&task, " M src/x.ts\n", " 1 file changed\n");
    assert_eq!(for_claude, for_codex);
    assert!(for_claude.contains("Add a Save button"));
    assert!(for_claude.contains("npm test"));
    assert!(for_claude.contains("sushi-report"));
}

#[test]
fn brief_includes_previous_attempt_failure_truncated() {
    let mut task = sample_task();
    let long_detail = "x".repeat(2000);
    task.fixture_data()
        .attempts
        .push(failed_implement_attempt(long_detail));
    let mut second = task.attempts[0].clone();
    second.n = 2;
    second.handoff = Some("tried bumping the timeout".into());
    task.fixture_data().attempts.push(second);
    let brief = build_brief(&task, "", "");
    assert!(brief.contains("Previous attempts"));
    assert!(brief.contains("Attempt 1 (claude-sonnet): failed"));
    assert!(brief.contains("  - handoff: tried bumping the timeout"));
    // An older failure is clipped to MAX_FAILURE_DETAIL; the latest one,
    // the one to fix now, keeps a larger budget.
    let failures: Vec<&str> = brief.lines().filter(|l| l.contains("failure:")).collect();
    assert!(failures[0].len() < 2000, "{}", failures[0].len());
    assert!(failures[1].contains(&"x".repeat(2000)));
}

#[test]
fn a_pass_that_marks_a_criterion_unmet_is_a_fail() {
    let r = parse_review(
        "```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[],\"criteria\":[{\"criterion\":\"Badge reads 0/N\",\"met\":false,\"evidence\":\"list still starts at 1\"},{\"criterion\":\"Test added\",\"met\":true}]}\n```",
    )
    .unwrap();
    assert_eq!(r.verdict, Verdict::Fail);
    assert_eq!(
        r.findings,
        vec!["Unmet criterion: Badge reads 0/N (list still starts at 1)".to_string()]
    );
    let unverified = parse_review(
        "```sushi-review\n{\"verdict\":\"PASS\",\"criteria\":[{\"criterion\":\"Looks right\",\"met\":null,\"evidence\":\"cannot run the UI\"},{\"name\":\"n\",\"met\":\"no\"}]}\n```",
    )
    .unwrap();
    assert_eq!(unverified.verdict, Verdict::Fail, "\"no\" is unmet");
    // The object's key order depends on serde_json's `preserve_order`, which another
    // workspace crate enables: compare the parsed text.
    assert_eq!(unverified.findings.len(), 2);
    assert_eq!(
        unverified.findings[0],
        "Not checked by review: Looks right (cannot run the UI)"
    );
    let unmet = unverified.findings[1]
        .strip_prefix("Unmet criterion: ")
        .expect("prefix");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(unmet).expect("json"),
        serde_json::json!({"name": "n", "met": "no"})
    );
    let only_unverified = parse_review(
        "```sushi-review\n{\"verdict\":\"PASS\",\"criteria\":[{\"criterion\":\"x\",\"met\":null}]}\n```",
    )
    .unwrap();
    assert_eq!(only_unverified.verdict, Verdict::Pass);
    let clean = parse_review(
        "```sushi-review\n{\"verdict\":\"PASS\",\"criteria\":[{\"criterion\":\"x\",\"met\":true}]}\n```",
    )
    .unwrap();
    assert_eq!(clean.verdict, Verdict::Pass);
}

#[test]
fn a_subtask_brief_names_its_targeted_check() {
    let mut task = sample_task();
    assert!(!build_brief(&task, "", "").contains("## Targeted check"));
    task.fixture_data().brief_check.check = Some("go test ./pkg/x".into());
    let brief = build_brief(&task, "", "");
    assert!(brief.contains("## Targeted check"));
    assert!(brief.contains("- `go test ./pkg/x`"));
}

#[test]
fn brief_previous_attempts_never_lists_the_plan_attempt() {
    let mut task = sample_task();
    task.fixture_data().attempts.push(Attempt {
        n: 1,
        stage: Stage::Plan,
        route_id: "claude-sonnet".into(),
        harness: Harness::Claude,
        model: "sonnet".into(),
        reason: "drafting".into(),
        session_id: None,
        pgid: None,
        started_at: 1,
        ended_at: Some(2),
        status: AttemptStatus::Passed,
        summary: Some("Drafted: Add a button".into()),
        handoff: None,
        declines: vec![],
        changed_files: vec![],
        verify: vec![],
        gate_blocks: 0,
        prefix_tokens: None,
        first_edit_at: None,
        review: None,
        failure: None,
        usage: None,
        cost_usd: None,
        cost_estimated: false,
        review_cost_usd: None,
        evidence: vec![],
        evidence_tree: None,
        review_tree: None,
        review_base: None,
        review_scope: None,
        evidence_base: None,
        evidence_from: None,
        advice: None,
        advisor_cost_usd: None,
        fingerprint: None,
        review_fingerprint: None,
        advisor_fingerprint: None,
        candidates: vec![],
        criteria_results: vec![],
        cause: None,
        diff_stat: None,
    });
    let brief = build_brief(&task, "", "");
    assert!(
        !brief.contains("Previous attempts"),
        "a plan-only history should never render the section at all: {brief}"
    );
}

#[test]
fn the_brief_lists_the_checklist_and_the_plan_notes() {
    let mut task = dependent_of(&[]);
    task.fixture_data().checklist = vec![
        crate::task::planning::ChecklistItem {
            text: "first".into(),
            check: "grep -q a f".into(),
            passes: true,
        },
        crate::task::planning::ChecklistItem {
            text: "second".into(),
            check: String::new(),
            passes: false,
        },
    ];
    task.fixture_data().plan_gate.findings = vec![crate::task::planning::PlanFinding {
        id: "review-0".into(),
        severity: crate::task::planning::Severity::Low,
        kind: "wrong approach".into(),
        text: "Prefer the hook".into(),
    }];
    let text = build_brief(&task, "", "");
    assert!(text.contains("## Checklist"), "{text}");
    assert!(
        text.contains("- [x] first -- check: `grep -q a f`"),
        "{text}"
    );
    assert!(text.contains("- [ ] second\n"), "{text}");
    assert!(text.contains("## Plan notes"), "{text}");
    assert!(
        text.contains("- LOW (wrong approach): Prefer the hook"),
        "{text}"
    );
    let plain = build_brief(&dependent_of(&[]), "", "");
    assert!(!plain.contains("## Checklist") && !plain.contains("## Plan notes"));
}

#[test]
fn a_check_naming_a_screenshot_or_an_artifacts_image_is_visual() {
    use crate::task::checks::commands::is_visual_criterion;
    assert!(is_visual_criterion(
        "x -- check: screenshot under artifacts/"
    ));
    assert!(is_visual_criterion(
        "x -- check: artifacts/panel.png shows it"
    ));
    assert!(!is_visual_criterion("x -- check: integration test"));
    assert!(is_visual_criterion("x -- check: an image under artifacts/"));
    assert!(is_visual_criterion("x -- check: artifacts/shot.gif"));
    assert!(!is_visual_criterion(
        "x -- check: artifacts/report.json lists it"
    ));
    assert!(!is_visual_criterion("a screenshot is nice"));
}

#[test]
fn the_implementer_brief_lists_indexed_criteria_and_gated_checks_only() {
    let brief = build_brief(&grounded_task(), "", "");
    assert!(brief.contains("- [0] Button visible\n- [1] Click saves settings\n"));
    assert!(brief.contains("## Checks\n"));
    assert!(brief.contains("- criterion [0]: `gated-cmd`"));
    assert!(!brief.contains("vacuous-cmd") && !brief.contains("env-cmd"));
    assert!(!brief.contains("SECRET-held-out-cmd"));
    assert!(brief.contains("```sushi-impossible"));
}
