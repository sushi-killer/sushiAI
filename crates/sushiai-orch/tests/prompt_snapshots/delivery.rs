use super::fixtures::{full_task, task, verify};
use serde_json::json;
use serde_json::Value;
use sushiai_orch::task::Task;

pub(super) fn renders() -> Vec<(&'static str, String)> {
    let bare = task(json!({ "criteria": [], "verify": [] }));
    let full = full_task();
    let diff = "--- a/src/settings.ts\n+++ b/src/settings.ts\n@@ -1 +1 @@\n-old\n+new";
    let passed = full.clone();
    let mut last = serde_json::to_value(&passed.attempts[2]).unwrap();
    last["n"] = json!(5);
    last["status"] = json!("passed");
    last["failure"] = Value::Null;
    last["verify"] = json!([
        verify("npm test -- settings", Some(0), "ok"),
        verify("npm run lint", None, "killed")
    ]);
    let mut passed_json = serde_json::to_value(&passed).unwrap();
    passed_json["attempts"].as_array_mut().unwrap().push(last);
    let passed: Task = serde_json::from_value(passed_json).unwrap();
    let pr_body = "## Summary\n\nAdds a Save button and tests for it.";
    vec![
        (
            "build_pr_draft_brief",
            sushiai_orch::delivery::brief::build_pr_draft_brief(&full, diff),
        ),
        (
            "build_pr_truth_brief",
            sushiai_orch::delivery::brief::build_pr_truth_brief(&passed, pr_body, diff),
        ),
        (
            "build_pr_truth_brief_no_results",
            sushiai_orch::delivery::brief::build_pr_truth_brief(&bare, pr_body, diff),
        ),
    ]
}
