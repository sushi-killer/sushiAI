use crate::engine::planning::prompt::{
    build_plan_brief, build_plan_retry_brief, build_subtask_plan_brief,
    build_subtask_plan_retry_brief,
};
use crate::policy::routing::Tier;
use crate::policy::variant::Variant;
use crate::task::questions::Risk;

use super::{parse_plan, Coverage};

#[test]
fn the_plan_format_has_a_screenshot_key_the_parser_reads() {
    let brief = build_plan_brief("r", &Variant::default(), "");
    assert!(brief.contains("\"screenshot\":\"\""), "{brief}");
    assert!(brief.contains("leave `screenshot` empty rather than inventing one"));
    let text = "```sushi-plan\n{\"title\":\"t\",\"screenshot\":\"npm run shot\"}\n```";
    let draft = parse_plan(text).expect("parses");
    assert_eq!(draft.screenshot.as_deref(), Some("npm run shot"));
}

#[test]
fn parse_plan_keeps_a_known_tier_and_drops_an_unknown_one() {
    let plan = |tier: &str| {
        parse_plan(&format!(
            "```sushi-plan\n{{\"title\":\"t\",\"tier\":{tier}}}\n```"
        ))
        .unwrap()
        .tier
    };
    assert_eq!(plan("\"hard\""), Some(Tier::Hard));
    assert_eq!(plan("\"extreme\""), None);
    assert_eq!(plan("3"), None);
}

#[test]
fn parse_plan_survives_an_inline_triple_backtick_inside_a_json_string() {
    // Captured from a real planner reply: the goal quoted
    // ```sushi-review``` inline, which used to close the fence early.
    let text = "Plan:\n\n```sushi-plan\n{\"title\":\"T\",\"goal\":\"no parseable ```sushi-review``` block\"}\n```\n";
    let plan = parse_plan(text).unwrap();
    assert!(plan.goal.contains("```sushi-review```"));
}

#[test]
fn parse_plan_reads_coverage_and_a_part_check_leniently() {
    let text = "```sushi-plan\n{\"title\":\"T\",\"coverage\":[{\"requirement\":\" item one \",\"criteria\":[0,\"x\",2]},{\"criteria\":[1]},{\"requirement\":\"\"},7,{\"requirement\":\"two\"}],\"subtasks\":[{\"key\":\"a\",\"request\":\"do a\",\"check\":\"cargo test\"},{\"key\":\"b\",\"request\":\"do b\",\"check\":null}]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(
        draft.coverage,
        vec![
            Coverage {
                requirement: "item one".into(),
                criteria: vec![0, 2]
            },
            Coverage {
                requirement: "two".into(),
                criteria: vec![]
            },
        ]
    );
    assert_eq!(draft.subtasks[0].check, "cargo test");
    assert_eq!(draft.subtasks[1].check, "");
    // Missing or malformed coverage is empty, never a parse failure.
    let bad = "```sushi-plan\n{\"title\":\"T\",\"coverage\":\"nope\"}\n```";
    assert!(parse_plan(bad).unwrap().coverage.is_empty());
}

#[test]
fn parse_plan_reads_subtasks_with_their_dependencies() {
    let text = "```sushi-plan\n{\"title\":\"T\",\"goal\":\"G\",\"subtasks\":[{\"key\":\"a\",\"title\":\"A\",\"request\":\"do a\"},{\"key\":\"b\",\"title\":\"B\",\"request\":\"do b\",\"dependsOn\":[\"a\"]}]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(draft.subtasks.len(), 2);
    assert_eq!(draft.subtasks[1].depends_on, vec!["a".to_string()]);
    assert!(parse_plan("```sushi-plan\n{\"title\":\"T\"}\n```")
        .unwrap()
        .subtasks
        .is_empty());
}

#[test]
fn parse_plan_reads_a_complete_part_draft_and_its_visual_criteria() {
    let text = "```sushi-plan\n{\"title\":\"T\",\"subtasks\":[{\"key\":\"a\",\"title\":\"A\",\"goal\":\"G\",\"criteria\":[\"c1\",{\"text\":\"c2\",\"visual\":true}],\"verify\":[\"true\"],\"finalVerify\":[\"x\"],\"tier\":\"hard\",\"request\":\"r\"},{\"key\":\"b\",\"request\":\"r\"}]}\n```";
    let plan = parse_plan(text).unwrap();
    let a = &plan.subtasks[0];
    assert_eq!(a.goal, "G");
    assert_eq!(a.criteria, vec!["c1", "c2"]);
    assert_eq!(a.visual_criteria, vec!["c2"]);
    assert_eq!(a.verify, vec!["true"]);
    assert_eq!(a.final_verify, vec!["x"]);
    assert_eq!(a.tier, Some(Tier::Hard));
    let b = &plan.subtasks[1];
    assert!(b.goal.is_empty() && b.criteria.is_empty() && b.tier.is_none());
}

#[test]
fn parse_plan_reads_criteria_objects_and_their_visual_mark() {
    let text = "```sushi-plan\n{\"title\":\"t\",\"criteria\":[\"plain\",{\"text\":\"looks right\",\"visual\":true},{\"text\":\"unmarked\"}]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(draft.criteria, vec!["plain", "looks right", "unmarked"]);
    assert_eq!(draft.visual_criteria, vec!["looks right"]);
}

#[test]
fn parse_plan_reads_title_goal_criteria_verify_and_questions() {
    let text = "```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add a dark theme toggle\",\"criteria\":[\"Toggle visible in settings\"],\"verify\":[\"npm test\"],\"questions\":[{\"text\":\"Which default?\",\"options\":[\"light\",\"dark\",\"system\"]}]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(draft.title, "Add dark mode");
    assert_eq!(
        draft.criteria,
        vec!["Toggle visible in settings".to_string()]
    );
    assert_eq!(draft.verify, vec!["npm test".to_string()]);
    assert_eq!(draft.questions.len(), 1);
    assert_eq!(draft.questions[0].text, "Which default?");
    assert_eq!(draft.questions[0].options.len(), 3);
}

#[test]
fn parse_plan_reads_a_question_risk_and_default_option() {
    let text = "```sushi-plan\n{\"title\":\"t\",\"goal\":\"g\",\"criteria\":[],\"verify\":[],\"questions\":[{\"text\":\"Pay for it?\",\"options\":[\"yes\",\"no\"],\"risk\":[\"money\"],\"defaultOption\":1}]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(draft.questions[0].risk, vec![Risk::Money]);
    assert_eq!(draft.questions[0].default_option, Some(1));
}

#[test]
fn parse_plan_returns_none_when_malformed_or_missing() {
    assert!(parse_plan("no fence here").is_none());
    assert!(parse_plan("```sushi-plan\nnot json\n```").is_none());
}

#[test]
fn parse_plan_defaults_missing_optional_fields() {
    let text = "```sushi-plan\n{\"title\":\"Add dark mode\",\"goal\":\"Add it\"}\n```";
    let draft = parse_plan(text).unwrap();
    assert!(draft.criteria.is_empty());
    assert!(draft.verify.is_empty());
    assert!(draft.questions.is_empty());
}

#[test]
fn default_plan_briefs_keep_the_plain_contract_and_split_boundary() {
    let variant = Variant {
        batch_questions: false,
        ..Variant::default()
    };
    let plan = build_plan_brief("Fix it", &variant, "");
    let subtask = build_subtask_plan_brief("Fix it", &variant, "");
    for brief in [&plan, &subtask] {
        assert!(brief.starts_with("## Request\n\nFix it\n\n## Instructions\n\n"));
        assert!(!brief.contains("\"recommended\""));
        let report = parse_plan(brief).unwrap();
        assert!(report.final_verify.is_empty());
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].criterion, 0);
        assert_eq!(report.held_out.as_ref().unwrap().criterion, 0);
        assert_eq!(report.tests.len(), 1);
    }
    assert_eq!(parse_plan(&plan).unwrap().subtasks.len(), 1);
    assert!(parse_plan(&subtask).unwrap().subtasks.is_empty());
    assert!(build_plan_retry_brief("Fix it", &variant, "").ends_with(&plan));
    assert!(build_subtask_plan_retry_brief("Fix it", &variant, "").ends_with(&subtask));
}

#[test]
fn a_plan_draft_parses_tests_leniently() {
    let text = "```sushi-plan\n{\"title\":\"t\",\"tests\":[{\"path\":\"a_test.rs\",\"content\":\"x\"},{\"path\":3},\"junk\"]}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(
        draft.tests,
        vec![crate::task::checks::types::PlannedTest {
            path: "a_test.rs".into(),
            content: "x".into()
        }]
    );
    let none = parse_plan("```sushi-plan\n{\"title\":\"t\",\"tests\":7}\n```").unwrap();
    assert!(none.tests.is_empty());
}

#[test]
fn plan_drafts_keep_usable_checks_and_drop_malformed_ones() {
    let text = "```sushi-plan\n{\"title\":\"t\",\"criteria\":[\"a\"],\"checks\":[{\"criterion\":0,\"run\":\"x\"},{\"criterion\":\"no\"},7],\"heldOut\":{\"criterion\":0,\"run\":\"h\"}}\n```";
    let draft = parse_plan(text).unwrap();
    assert_eq!(draft.checks.len(), 1);
    assert_eq!(draft.held_out.unwrap().run, "h");
    let bad = parse_plan("```sushi-plan\n{\"title\":\"t\",\"checks\":\"none\",\"heldOut\":3}\n```")
        .unwrap();
    assert!(bad.checks.is_empty() && bad.held_out.is_none());
}
