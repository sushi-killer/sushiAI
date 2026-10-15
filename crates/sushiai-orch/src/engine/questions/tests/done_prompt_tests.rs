use crate::engine::questions::wake::overseer_prompt;
use crate::task::TaskStatus;

#[test]
fn the_overseer_reports_a_done_task_and_never_opens_its_pull_request() {
    let task = crate::engine::test_support::task_with_status(TaskStatus::Done);
    let prompt = overseer_prompt("overseer_done", &task);
    assert!(!prompt.contains("task_pr"), "{prompt}");
}
