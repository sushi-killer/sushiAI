use crate::engine::planning::subtasks::{prepare_subtasks, split_into_subtasks};
use crate::engine::test_support::{
    attempt_with_failure, git_in, init_git_repo, task_with_status, test_app,
};
use crate::task::{Task, TaskStatus};
use serde_json::json;
fn prose_and_real_checks() -> serde_json::Value {
    json!([
        {"criterion": 0, "run": "cargo test -p x foo"},
        {"criterion": 1, "run": "Amendment{criteria: Some(vec![...])}.apply(&mut task)"},
    ])
}

#[test]
fn store_draft_keeps_no_prose_check() {
    let (app, dir) = test_app();
    let mut task = task_with_status(TaskStatus::Drafting);
    task.fixture_data().attempts = vec![attempt_with_failure(1, "failure")];
    let draft: crate::engine::planning::draft::PlanDraft = serde_json::from_value(json!({
        "title": "T",
        "goal": "G",
        "criteria": ["one", "two"],
        "checks": prose_and_real_checks(),
        "heldOut": {"criterion": 1, "run": "the parser keeps the last row"},
    }))
    .unwrap();
    task.fixture_data().worktree = dir.path().to_string_lossy().into_owned();
    let settings = app.settings.read().unwrap().clone();
    crate::engine::planning::records::store_draft(&mut task, 0, &draft, &settings);
    assert_eq!(task.criteria.len(), 2);
    assert_eq!(task.checks.len(), 1);
    assert_eq!(task.checks[0].run, "cargo test -p x foo");
    assert!(task.held_out.is_none());
    let lines = task
        .decisions
        .iter()
        .filter(|d| d.contains("has no check"))
        .count();
    assert_eq!(lines, 2, "{:?}", task.decisions);
}

#[tokio::test]
async fn a_drafted_subtask_part_keeps_no_prose_check() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = task_with_status(TaskStatus::Running);
    parent.fixture_data().repo = repo.to_string_lossy().into();
    parent.fixture_data().worktree = parent.repo.clone();
    parent.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    parent.fixture_data().base_ref = crate::git::branches::branch_of(&repo, "HEAD");
    parent.fixture_data().branch = parent.base_ref.clone().unwrap_or_default();
    app.store.save_task(&parent).unwrap();
    let part = |key: &str, deps: &[&str]| -> crate::engine::planning::draft::PlanSubtask {
        serde_json::from_value(json!({
            "key": key,
            "title": key,
            "request": format!("do {key}"),
            "goal": "g",
            "criteria": ["one", "two"],
            "verify": ["npm test"],
            "check": "npm test",
            "dependsOn": deps,
            "paths": [format!("src/{key}.rs")],
            "checks": prose_and_real_checks(),
            "heldOut": {"criterion": 1, "run": "the parser keeps the last row"},
        }))
        .unwrap()
    };
    let plan = prepare_subtasks(&[part("a", &[]), part("b", &["a"])]);
    split_into_subtasks(&app, &parent.id, &plan, false)
        .await
        .unwrap();
    let children: Vec<Task> = app
        .store
        .list_tasks()
        .unwrap()
        .into_iter()
        .filter(|t| t.parent.as_deref() == Some(parent.id.as_str()))
        .collect();
    assert_eq!(children.len(), 2);
    for child in &children {
        assert!(child.criteria.len() >= 2);
        assert_eq!(child.checks.len(), 1, "{:?}", child.checks);
        assert_eq!(child.checks[0].run, "cargo test -p x foo");
        assert!(child.held_out.is_none());
        let lines = child
            .decisions
            .iter()
            .filter(|d| d.contains("has no check"))
            .count();
        assert_eq!(lines, 2, "{:?}", child.decisions);
    }
}
