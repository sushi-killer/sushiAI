use crate::engine::test_support::{init_git_repo, task_with_status, test_app};
use crate::task::{Check, TaskStatus};
use serde_json::json;
const PROSE_RUN: &str = "Amendment{criteria: Some(vec![...])}.apply(&mut task)";

fn check_of(criterion: usize, run: &str) -> Check {
    Check {
        criterion,
        run: run.into(),
        baseline: None,
    }
}

fn no_check_lines(decisions: &[String]) -> Vec<&String> {
    decisions
        .iter()
        .filter(|d| d.contains("has no check"))
        .collect()
}

#[tokio::test]
async fn task_amend_keeps_no_prose_check() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().criteria = vec!["one".into(), "two".into()];
    task.fixture_data().worktree = std::env::temp_dir().to_string_lossy().into();
    app.store.save_task(&task).unwrap();
    app.dispatch(
        "task.amend",
        json!({
            "id": task.id,
            "checks": [
                {"criterion": 0, "run": "cargo test -p x foo"},
                {"criterion": 1, "run": PROSE_RUN},
            ],
            "heldOut": {"criterion": 1, "run": "the parser keeps the last row"},
        }),
    )
    .await
    .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.checks, vec![check_of(0, "cargo test -p x foo")]);
    assert_eq!(saved.held_out, None);
    assert_eq!(saved.criteria, vec!["one", "two"]);
    let lines = no_check_lines(&saved.decisions);
    assert_eq!(lines.len(), 2, "{:?}", saved.decisions);
    assert!(lines[0].contains("criterion 1") && lines[0].contains(PROSE_RUN));
}

#[tokio::test]
async fn task_amend_gates_in_the_repo_when_the_worktree_is_gone() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut task = task_with_status(TaskStatus::Queued);
    task.fixture_data().criteria = vec!["one".into(), "two".into()];
    task.fixture_data().repo = repo.to_string_lossy().into();
    task.fixture_data().worktree = dir.path().join("gone").to_string_lossy().into();
    app.store.save_task(&task).unwrap();
    app.dispatch(
        "task.amend",
        json!({
            "id": task.id,
            "checks": [
                {"criterion": 0, "run": "npm test"},
                {"criterion": 1, "run": PROSE_RUN},
            ],
            "heldOut": {"criterion": 0, "run": "cargo test -p x foo"},
        }),
    )
    .await
    .unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.checks, vec![check_of(0, "npm test")]);
    assert_eq!(saved.held_out, Some(check_of(0, "cargo test -p x foo")));
    assert_eq!(no_check_lines(&saved.decisions).len(), 1);
}

#[tokio::test]
async fn task_create_keeps_no_prose_check() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let created = app
        .dispatch(
            "task.create",
            json!({
                "repo": repo,
                "title": "Do thing",
                "goal": "Do the thing",
                "criteria": ["one", "two"],
                "checks": [
                    {"criterion": 0, "run": "npm test"},
                    {"criterion": 1, "run": PROSE_RUN},
                ],
                "start": false,
            }),
        )
        .await
        .unwrap();
    let saved = app
        .store
        .load_task(created["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(saved.checks, vec![check_of(0, "npm test")]);
    assert_eq!(saved.criteria.len(), 2);
    assert_eq!(
        no_check_lines(&saved.decisions).len(),
        1,
        "{:?}",
        saved.decisions
    );
}
