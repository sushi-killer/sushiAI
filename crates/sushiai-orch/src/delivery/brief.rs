use crate::prompts::text::untrusted_block;
use crate::prompts::{brief_text, fill};
use crate::task::attempt::{AttemptStatus, Stage};
use crate::task::state::Task;

/// What the pull request drafter is asked: read the repo's own contributor
/// rules and draft the title and body of the pull request for the finished
/// task. Read-only; the reply is one JSON object.
pub fn build_pr_draft_brief(task: &Task, diff: &str) -> String {
    let mut out = format!("{}\n\n## Task\n\n", brief_text("pr_draft"));
    out.push_str(&task.title);
    out.push_str("\n\n");
    out.push_str(&task.goal);
    out.push_str("\n\n## Acceptance criteria\n\n");
    for c in &task.criteria {
        out.push_str("- ");
        out.push_str(c);
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&fill(
        brief_text("pr_draft_branch"),
        &[("branch", &task.branch)],
    ));
    out.push('\n');
    if let Some(report) = task.report.as_deref().filter(|r| !r.trim().is_empty()) {
        out.push_str("\n## Report\n\n");
        out.push_str(&untrusted_block("The task's report", report));
    }
    out.push_str("\n## Diff against the base\n\n");
    out.push_str(&untrusted_block("The task's diff", diff));
    out
}

/// What the body check is asked: compare the drafted pull request body with
/// the final diff and the check results, and name only what is false. Read-only;
/// the reply is one JSON object.
pub fn build_pr_truth_brief(task: &Task, body: &str, diff: &str) -> String {
    let mut out = format!("{}\n\n## Check results\n\n", brief_text("pr_truth"));
    out.push_str(&verify_results(task));
    out.push_str("\n## The drafted body\n\n");
    out.push_str(&untrusted_block("The drafted body", body));
    out.push_str("\n## Diff against the base\n\n");
    out.push_str(&untrusted_block("The task's diff", diff));
    out
}

/// The commands the last passing attempt ran, with their exit codes.
fn verify_results(task: &Task) -> String {
    let passed = task
        .attempts
        .iter()
        .rev()
        .find(|a| a.stage == Stage::Implement && a.status == AttemptStatus::Passed);
    let mut out = String::new();
    for v in passed.map_or(&[][..], |a| a.verify.as_slice()) {
        let code = v.code.map_or_else(|| "none".to_string(), |c| c.to_string());
        out.push_str(&format!("- `{}`: exit code {code}\n", v.command));
    }
    if out.is_empty() {
        out.push_str("No check results were recorded.\n");
    }
    out
}
