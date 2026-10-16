use crate::prompts::engine_text;
use crate::task::Task;

fn context(task: &Task, path: &str, content: &str, reason: &str) -> String {
    let criteria = task
        .criteria
        .iter()
        .map(|c| format!("- {c}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\n{}\n{}\n{}\n",
        crate::prompts::text::untrusted_block("The task goal", &task.goal),
        crate::prompts::text::untrusted_block("The acceptance criteria", &criteria),
        crate::prompts::text::untrusted_block(&format!("The test {path}"), content),
        crate::prompts::text::untrusted_block("The implementer's claim", reason),
    )
}

/// The judge's brief: is the test at `path` (now `content`) wrong, as the
/// implementer's `reason` claims?
pub(in crate::engine) fn judge_brief(
    task: &Task,
    path: &str,
    content: &str,
    reason: &str,
) -> String {
    format!(
        "{}\n\n{}{}\n",
        engine_text("test_claim_judge"),
        context(task, path, content, reason),
        engine_text("test_claim_judge_reply")
    )
}

/// The planner's brief to rewrite the test at `path` the judge found wrong.
pub(in crate::engine) fn rewrite_brief(
    task: &Task,
    path: &str,
    content: &str,
    reason: &str,
) -> String {
    format!(
        "{}\n\n{}{}\n",
        engine_text("test_claim_rewrite"),
        context(task, path, content, reason),
        engine_text("test_claim_rewrite_reply")
    )
}
