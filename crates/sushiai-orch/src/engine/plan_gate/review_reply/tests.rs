use super::*;
use crate::task::TaskStatus;

#[test]
fn a_review_reply_must_match_the_schema() {
    let found = parse_review(
        "Here:\n```json\n{\"findings\":[{\"kind\":\"wrong-split\",\"severity\":\"critical\",\"text\":\" a  b \"},{\"kind\":\"report-only\",\"criterion\":1,\"severity\":\"high\",\"text\":\"c\"}]}\n```",
        2,
    )
    .unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].kind, ReviewKind::WrongSplit);
    assert_eq!(found[1].criterion, Some(1));
    assert!(parse_review("{\"findings\": []}", 2).unwrap().is_empty());
    for bad in [
        "no json",
        "{\"findings\": 3}",
        // A kind outside the five, a severity outside the three, no text,
        // an index the draft does not have, a report-only without one.
        "{\"findings\":[{\"kind\":\"Wrong split\",\"severity\":\"critical\",\"text\":\"x\"}]}",
        "{\"findings\":[{\"kind\":\"untestable\",\"severity\":\"low\",\"text\":\"x\"}]}",
        "{\"findings\":[{\"kind\":\"untestable\",\"severity\":\"high\"}]}",
        "{\"findings\":[{\"kind\":\"untestable\",\"criterion\":2,\"severity\":\"high\",\"text\":\"x\"}]}",
        "{\"findings\":[{\"kind\":\"report-only\",\"severity\":\"high\",\"text\":\"x\"}]}",
    ] {
        assert!(parse_review(bad, 2).is_err(), "{bad}");
    }
}

#[test]
fn only_a_typed_report_only_finding_names_a_criterion_for_removal() {
    let found = parse_review(
        "{\"findings\":[{\"kind\":\"untestable\",\"criterion\":0,\"severity\":\"high\",\"text\":\"x\"},{\"kind\":\"report-only\",\"criterion\":2,\"severity\":\"medium\",\"text\":\"y\"}]}",
        3,
    )
    .unwrap();
    assert_eq!(report_only_criteria(&found), vec![2]);
    assert!(report_only_criteria(&parse_review("{\"findings\": []}", 3).unwrap()).is_empty());
}

#[test]
fn a_report_only_criterion_with_a_shared_check_is_kept_as_a_finding() {
    use crate::engine::test_support::{task_with_status, test_app};
    let (app, _dir) = test_app();
    let settings = app.settings.read().unwrap().clone();
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().criteria = vec![
        "Shared -- check: npm run ci".into(),
        "Own -- check: test -f made.txt".into(),
    ];
    task.fixture_data().request = Some("Run `npm run ci`".into());
    task.fixture_data().planner_tests.sources = vec![crate::task::SourcedCheck {
        command: "test -f made.txt".into(),
        source: crate::task::CheckSource::Planner,
    }];
    task.fixture_data().planner_tests.files = vec![crate::task::PlannedTest {
        path: "made.txt".into(),
        content: String::new(),
    }];
    let found = parse_review(
        "{\"findings\":[{\"kind\":\"report-only\",\"criterion\":0,\"severity\":\"critical\",\"text\":\"a\"},{\"kind\":\"report-only\",\"criterion\":1,\"severity\":\"medium\",\"text\":\"b\"}]}",
        2,
    )
    .unwrap();
    let (removals, review) = split_review(&settings, &task, &found);
    assert_eq!(removals, vec![1]);
    assert_eq!(review.len(), 1);
    assert_eq!(review[0].severity, Severity::High);
    assert_eq!(review[0].kind, "report-only");
}
