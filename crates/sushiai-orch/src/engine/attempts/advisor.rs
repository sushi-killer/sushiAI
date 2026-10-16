use crate::engine::attempts::brief::MAX_LATEST_FAILURE_DETAIL;
use crate::prompts::brief_text;
use crate::prompts::text::{clip_middle, untrusted_block};
use crate::task::state::Task;

/// The advisor's answer for the next attempt, as data.
pub fn advisor_block(advice: &str) -> String {
    format!(
        "## Advisor\n\n{}\n",
        untrusted_block(brief_text("advisor_label"), advice)
    )
}

/// What the advisor is asked: diagnose the failed attempt and name one next
/// step, in at most 1500 characters. Read-only.
pub fn build_advisor_brief(task: &Task, diff: &str, failure_detail: &str) -> String {
    let mut out = format!("{}\n\n## Task\n\n", brief_text("advisor"));
    out.push_str(&task.title);
    out.push_str("\n\n");
    out.push_str(&task.goal);
    out.push_str("\n\n## Acceptance criteria\n\n");
    for c in &task.criteria {
        out.push_str("- ");
        out.push_str(c);
        out.push('\n');
    }
    out.push_str("\n## Failure detail\n\n");
    out.push_str(&untrusted_block(
        "How the attempt failed",
        &clip_middle(failure_detail, MAX_LATEST_FAILURE_DETAIL),
    ));
    out.push_str("\n## Diff of the failed attempt\n\n```diff\n");
    out.push_str(diff);
    out.push_str("\n```\n");
    out
}
