use super::*;
use crate::task::TaskStatus;

#[test]
fn plan_gate_moves_whole_repo_commands_to_final_verify_when_another_remains() {
    use crate::engine::test_support::{attempt_with_failure, task_with_status, test_app};
    let (app, dir) = test_app();
    let mut settings = app.settings.read().unwrap().clone();
    settings.review.clear();
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
    let mut attempt = attempt_with_failure(1, "drafting");
    attempt.stage = Stage::Plan;
    attempt.status = AttemptStatus::Running;
    attempt.failure = None;
    task.fixture_data().attempts.push(attempt);
    let draft = crate::engine::planning::draft::parse_plan(
        "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\",\"cargo test -p x --lib\"],\"finalVerify\":[\"npm run ci\"]}\n```",
    ).unwrap();
    store_draft(&mut task, 0, &draft, &settings);
    assert_eq!(task.verify, vec!["cargo test -p x --lib"]);
    assert_eq!(task.final_verify, vec!["npm run ci"]);
    assert!(task
        .decisions
        .iter()
        .any(|d| d.contains("npm run ci") && d.contains("moved to finalVerify")));

    // A sole suite also runs once as the final gate, with review off.
    let sole = crate::engine::planning::draft::parse_plan(
        "```sushi-plan\n{\"title\":\"Run checks\",\"verify\":[\"npm run ci\"]}\n```",
    )
    .unwrap();
    store_draft(&mut task, 0, &sole, &settings);
    assert!(task.verify.is_empty());
    assert_eq!(task.final_verify, vec!["npm run ci"]);
}
