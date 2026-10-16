use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{Baseline, TaskStatus};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

struct Release(PathBuf);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

#[tokio::test]
async fn an_owner_rpc_amendment_rejects_its_stale_baseline_after_a_new_owner_plan() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    task.fixture_data().criteria = vec!["original meaning".into()];
    app.store.save_task(&task).unwrap();
    let entered = dir.path().join("entered");
    let released = dir.path().join("released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; exit 1",
        entered.display(),
        released.display()
    );
    let first_app = app.clone();
    let first_id = task.id.clone();
    let first = tokio::spawn(async move {
        first_app.dispatch("task.amend", json!({"id": first_id, "criteria": ["captured meaning"], "checks": [{"criterion": 0, "run": command}]})).await
    });
    let reached = tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if reached.is_err() {
        std::fs::write(&released, "release").unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), first).await;
        panic!("baseline did not reach its barrier: {result:?}");
    }
    let amended = tokio::time::timeout(Duration::from_secs(5), app.dispatch("task.amend", json!({
        "id": task.id, "criteria": ["current owner meaning"], "verify": ["true"], "finalVerify": ["true"],
        "checks": [{"criterion": 0, "run": "true", "baseline": "pass"}]
    }))).await.expect("a baseline await must release the task lock").unwrap();
    assert_eq!(amended["pending"], false);
    let current = app.store.load_task(&task.id).unwrap().unwrap();
    let before = serde_json::to_value(&current).unwrap();
    std::fs::write(&released, "release").unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("stale"), "{refused}");
    let after = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(after.criteria, ["current owner meaning"]);
    assert_eq!(after.checks[0].run, "true");
    assert_eq!(after.checks[0].baseline, Some(Baseline::Pass));
    assert_eq!(serde_json::to_value(after).unwrap(), before);
}

#[tokio::test]
async fn a_malformed_rpc_amendment_source_cannot_claim_owner_authority() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().criteria = vec!["shared behavior".into()];
    task.fixture_data().verify = vec!["shared-check".into()];
    app.store.save_task(&task).unwrap();
    for source in [
        json!(true),
        json!(7),
        json!({"owner": true}),
        json!(["owner"]),
    ] {
        let before = serde_json::to_value(app.store.load_task(&task.id).unwrap().unwrap()).unwrap();
        let error = app
            .dispatch(
                "task.amend",
                json!({"id": task.id, "verify": [], "source": source}),
            )
            .await
            .unwrap_err();
        assert!(error.contains("source must be a string"), "{error}");
        assert_eq!(
            serde_json::to_value(app.store.load_task(&task.id).unwrap().unwrap()).unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn two_fresh_owner_drop_requests_may_each_target_current_criterion_zero() {
    use crate::engine::run_control::{TaskControl, TaskSignals};
    use crate::task::{AskedBy, Check, Question, QuestionKind};
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Waiting);
    task.fixture_data().criteria = vec!["first".into(), "second".into(), "third".into()];
    task.fixture_data().checks = vec![
        Check {
            criterion: 0,
            run: "first-check".into(),
            baseline: None,
        },
        Check {
            criterion: 1,
            run: "second-check".into(),
            baseline: None,
        },
        Check {
            criterion: 2,
            run: "third-check".into(),
            baseline: None,
        },
    ];
    let question = || {
        let mut question = Question::new(
            "Current criterion zero is impossible",
            vec!["drop criterion".into(), "retry".into()],
            QuestionKind::Impossible,
            AskedBy::Implement,
            crate::clock::now_ms(),
        );
        question.criterion = Some(0);
        question
    };
    task.ask(question(), crate::clock::now_ms());
    app.store.save_task(&task).unwrap();
    // An existing control with no parked sender exercises the synchronous RPC
    // branch without starting an unrelated native implement attempt.
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: TaskSignals::new(),
            handle: tokio::spawn(async {}),
        },
    );
    app.dispatch(
        "task.answer",
        json!({"id": task.id, "answer": "drop criterion"}),
    )
    .await
    .unwrap();
    let first = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(first.criteria, ["second", "third"]);
    assert_eq!(
        first
            .checks
            .iter()
            .map(|check| check.criterion)
            .collect::<Vec<_>>(),
        [0, 1]
    );
    app.update_task(&task.id, |task| {
        task.ask(question(), crate::clock::now_ms())
    })
    .unwrap();
    app.dispatch(
        "task.answer",
        json!({"id": task.id, "answer": "drop criterion"}),
    )
    .await
    .unwrap();
    let second = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(second.criteria, ["third"]);
    assert_eq!(
        (second.checks[0].criterion, second.checks[0].run.as_str()),
        (0, "third-check")
    );
    let control = app.controls.lock().unwrap().remove(&task.id).unwrap();
    control.handle.await.unwrap();
}

fn parent_graph(
    app: &std::sync::Arc<crate::engine::state::App>,
    dir: &tempfile::TempDir,
    verify: String,
) -> crate::task::Task {
    let repo = dir.path().join("repo");
    let worktree = dir.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut parent = task_with_status(TaskStatus::Running);
    let created =
        crate::git::worktree::create_worktree(&repo, &parent.branch, &worktree, "HEAD").unwrap();
    parent.fixture_data().repo = repo.to_string_lossy().into_owned();
    parent.fixture_data().worktree = worktree.to_string_lossy().into_owned();
    parent.fixture_data().base_sha = created.base_sha;
    parent.fixture_data().base_ref = crate::git::branches::branch_of(&repo, "HEAD");
    parent.fixture_data().verify = vec![verify];
    let mut child = task_with_status(TaskStatus::Done);
    child.fixture_data().parent = Some(parent.id.clone());
    child.fixture_data().repo = parent.repo.clone();
    app.store.save_task(&parent).unwrap();
    app.store.save_task(&child).unwrap();
    parent
}

#[tokio::test]
async fn an_owner_edit_received_during_parent_checks_is_checked_before_delivery() {
    use crate::engine::graph::parent::{run_parent, ParentEnd};
    use crate::engine::run_control::{TaskControl, TaskSignals};
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let entered = dir.path().join("parent-entered");
    let released = dir.path().join("parent-released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done",
        entered.display(),
        released.display()
    );
    let parent = parent_graph(&app, &dir, command);
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        parent.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let running_app = app.clone();
    let parent_id = parent.id.clone();
    let running = tokio::spawn(async move { run_parent(&running_app, &parent_id, &signals).await });
    let reached = tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if reached.is_err() {
        std::fs::write(&released, "release").unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(10), running).await;
        panic!("parent check did not reach its barrier");
    }
    let amended = app
        .dispatch(
            "task.amend",
            json!({"id": parent.id, "verify": ["touch current-plan-ran"]}),
        )
        .await
        .unwrap();
    assert_eq!(amended["pending"], true);
    std::fs::write(&released, "release").unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .unwrap()
            .unwrap(),
        ParentEnd::Finished
    ));
    let saved = app.store.load_task(&parent.id).unwrap().unwrap();
    assert_eq!(saved.verify, ["touch current-plan-ran"]);
    assert_eq!(saved.status, TaskStatus::Done);
    assert!(
        crate::git::history::tree_files(std::path::Path::new(&parent.repo), &saved.branch)
            .unwrap()
            .contains(&"current-plan-ran".to_string())
    );
    let control = app.controls.lock().unwrap().remove(&parent.id).unwrap();
    assert!(control.signals.pending_amend.lock().unwrap().is_none());
    control.handle.await.unwrap();
}
