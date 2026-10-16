use super::*;
use crate::engine::plan_gate::test_support::part;
use crate::task::{Check, PlannedTest, TaskStatus};

#[test]
fn the_parent_plan_review_sees_the_complete_drafts_its_children_will_run() {
    let task = crate::engine::test_support::task_with_status(TaskStatus::Drafting);
    let mut child = part("child", &[], &["child.txt"], "test -f child.txt");
    child.title = "Write child".into();
    child.goal = "Keep the requested behaviour".into();
    child.criteria = vec!["Child behaves correctly".into()];
    child.verify = vec!["test -f child.txt".into()];
    child.final_verify = vec!["cargo test -p child".into()];
    child.tests = vec![PlannedTest {
        path: "child.test.ts".into(),
        content: "assert(child())".into(),
    }];
    child.screenshot = Some("npm run screenshot:child".into());
    child.held_out = Some(Check {
        criterion: 0,
        run: "secret-hidden-check".into(),
        baseline: None,
    });
    let prompt = build_review_prompt(&task, "Build the child", &[child]);
    let json: serde_json::Value = serde_json::from_str(
        prompt
            .strip_prefix(engine_text("plan_gate_review"))
            .unwrap()
            .trim(),
    )
    .unwrap();
    let child = &json["subtasks"][0];
    assert!(!prompt.contains("secret-hidden-check"));
    assert_eq!(child["heldOutCriterion"], 0);
    assert_eq!(child["goal"], "Keep the requested behaviour");
    assert_eq!(child["criteria"], json!(["Child behaves correctly"]));
    assert_eq!(child["verify"], json!(["test -f child.txt"]));
    assert_eq!(child["finalVerify"], json!(["cargo test -p child"]));
    assert_eq!(child["tests"][0]["content"], "assert(child())");
    assert_eq!(child["screenshot"], "npm run screenshot:child");
}
