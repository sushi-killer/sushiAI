use super::Tail;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::TaskSignals;
use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::task::{AttemptStatus, Stage, TaskStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct ReleaseCommit(PathBuf);

impl Drop for ReleaseCommit {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

struct CommittedLanding {
    app: Arc<App>,
    _home: tempfile::TempDir,
    _repository: tempfile::TempDir,
    task_id: String,
    head: String,
}

fn committed_landing(receipt: &str) -> CommittedLanding {
    let (app, home) = test_app();
    app.settings.write().unwrap().pr_on_done_off = true;
    app.settings.write().unwrap().auto_answer = false;
    app.store
        .save_settings(&app.settings.read().unwrap())
        .unwrap();
    let repository = tempfile::tempdir().unwrap();
    let repo = repository.path().join("repo");
    let worktree = repository.path().join("worktree");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut task = task_with_status(TaskStatus::Running);
    let created =
        crate::git::worktree::create_worktree(&repo, &task.branch, &worktree, "HEAD").unwrap();
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().worktree = worktree.to_string_lossy().into_owned();
    task.fixture_data().base_sha = created.base_sha;
    task.fixture_data().variant = Some(crate::policy::variant::Variant {
        land: false,
        ..Default::default()
    });
    let route = app.settings.read().unwrap().routes[0].clone();
    let index = crate::task::attempts::start(&mut task, Stage::Implement, &route, "fixture", 1);
    crate::task::attempts::end(
        &mut task,
        index,
        crate::task::attempts::AttemptEnd::Passed,
        2,
    );
    task.fixture_data().attempts[index].cost_usd = Some(0.02);
    task.fixture_data().cost_usd = 0.02;
    assert!(task.begin_delivery(3));
    app.store.save_task(&task).unwrap();
    std::fs::write(worktree.join("delivered.txt"), "synthetic completed work\n").unwrap();
    git_in(&worktree, &["add", "delivered.txt"]);
    let task_trailer = format!(
        "Task-Id: {}",
        if receipt == "wrong-task" {
            "another-task"
        } else {
            &task.id
        }
    );
    let attempt_trailer = if receipt == "wrong-attempt" {
        "Attempt: 2"
    } else {
        "Attempt: 1"
    };
    git_in(
        &worktree,
        &[
            "commit",
            "-q",
            "-m",
            "Delivered synthetic work",
            "--trailer",
            &task_trailer,
            "--trailer",
            attempt_trailer,
        ],
    );
    let head = git_in(&worktree, &["rev-parse", "HEAD"]);
    assert!(git_in(&worktree, &["status", "--porcelain"]).is_empty());
    CommittedLanding {
        app,
        _home: home,
        _repository: repository,
        task_id: task.id.clone(),
        head,
    }
}

async fn finish_saved_landing(
    fixture: &CommittedLanding,
    expected_attempt: AttemptStatus,
) -> crate::task::Task {
    let app = App::new(
        fixture.app.data_dir.clone(),
        fixture.app.home.clone(),
        fixture.app.exe_path.clone(),
    )
    .unwrap();
    app.assume_harnesses_installed();
    let mut task = app.store.load_task(&fixture.task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Landing);
    assert_eq!(task.attempts[0].status, expected_attempt);
    let worktree = task.worktree.clone();
    let run_dir = app.store.run_dir(&task.id, 1);
    std::fs::create_dir_all(&run_dir).unwrap();
    let mut attempt_budget = 1;
    let mut permit = None;
    let signals = TaskSignals::new();
    assert!(matches!(
        AttemptFiles::new(0, 1, PathBuf::from(worktree), run_dir)
            .finish(
                &app,
                &mut task,
                &mut attempt_budget,
                &mut QuestionSlot::new(&signals, &mut permit)
            )
            .await,
        Tail::Return
    ));
    let saved = app.store.load_task(&fixture.task_id).unwrap().unwrap();
    assert_eq!(saved.attempts.len(), 1);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.02));
    assert_eq!(saved.cost_usd, 0.02);
    assert_eq!(
        git_in(Path::new(&saved.repo), &["rev-parse", &saved.branch]),
        fixture.head
    );
    saved
}

#[tokio::test]
async fn restarting_after_a_matching_commit_finishes_without_another_commit_or_attempt() {
    let fixture = committed_landing("matching");
    let task = finish_saved_landing(&fixture, AttemptStatus::Passed).await;
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.attempts[0].status, AttemptStatus::Passed);
    assert!(task.attempts[0].failure.is_none());
}

#[tokio::test]
async fn restarting_on_a_clean_tree_refuses_another_task_or_attempt_commit_receipt() {
    for receipt in ["wrong-task", "wrong-attempt"] {
        let fixture = committed_landing(receipt);
        let task = finish_saved_landing(&fixture, AttemptStatus::Passed).await;
        assert_eq!(task.status, TaskStatus::Failed, "{receipt}");
        assert_eq!(
            task.attempts[0].failure.as_ref().unwrap().kind,
            crate::task::FailureKind::Error
        );
    }
}

#[tokio::test]
async fn restart_recovery_keeps_a_completed_native_run_for_its_pending_delivery() {
    let fixture = committed_landing("matching");
    let app = &fixture.app;
    let mut task = app.store.load_task(&fixture.task_id).unwrap().unwrap();
    task.fixture_data().attempts[0].status = AttemptStatus::Running;
    task.fixture_data().attempts[0].ended_at = None;
    app.store.save_task(&task).unwrap();
    let run_dir = app.store.run_dir(&task.id, 1);
    std::fs::create_dir_all(&run_dir).unwrap();
    let files = crate::harness::run_files::RunFiles::of(&run_dir.join("events.jsonl"));
    std::fs::write(
        &files.raw,
        "{\"type\":\"result\",\"total_cost_usd\":0.02,\"result\":\"done\"}\n",
    )
    .unwrap();
    std::fs::write(files.exit, "0").unwrap();
    let interrupted = crate::engine::recovery::interrupted::tasks(
        &app.store,
        |task, index| app.run_fate(task, index),
        |_, _| panic!("a completed native run must not be settled again"),
    )
    .unwrap();
    assert!(interrupted.is_empty());
    let task = finish_saved_landing(&fixture, AttemptStatus::Running).await;
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.attempts[0].status, AttemptStatus::Passed);
    assert!(task.attempts[0].failure.is_none());
}

#[tokio::test]
async fn stopping_during_a_branch_commit_keeps_the_owner_stop_after_the_commit_finishes() {
    use crate::engine::run_control::{TaskControl, TaskSignals};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    let fixture = committed_landing("matching");
    let app = fixture.app.clone();
    let id = fixture.task_id.clone();
    let task = app.store.load_task(&id).unwrap().unwrap();
    let worktree = PathBuf::from(&task.worktree);
    let entered = fixture._home.path().join("commit-entered");
    let released = fixture._home.path().join("commit-released");
    let _release = ReleaseCommit(released.clone());
    let filter = fixture._home.path().join("commit-filter");
    std::fs::write(
        &filter,
        format!(
            "#!/bin/sh\ncat\ntouch '{}'\nwhile ! test -f '{}'; do sleep 0.01; done\n",
            entered.display(),
            released.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&filter, std::fs::Permissions::from_mode(0o700)).unwrap();
    git_in(
        &worktree,
        &["config", "filter.qa-stop.clean", filter.to_str().unwrap()],
    );
    std::fs::write(
        worktree.join(".gitattributes"),
        "delivered.txt filter=qa-stop\n",
    )
    .unwrap();
    std::fs::write(worktree.join("delivered.txt"), "new synthetic delivery\n").unwrap();
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let delivery_app = app.clone();
    let delivery_id = id.clone();
    let delivery = tokio::spawn(async move {
        let mut task = task;
        let run_dir = delivery_app.store.run_dir(&delivery_id, 1);
        std::fs::create_dir_all(&run_dir).unwrap();
        AttemptFiles::new(0, 1, worktree, run_dir)
            .finish(
                &delivery_app,
                &mut task,
                &mut 1,
                &mut QuestionSlot::new(&signals, &mut None),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the branch commit must reach its filter barrier");
    app.dispatch("task.stop", json!({"id": id})).await.unwrap();
    let stopped = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(stopped.status, TaskStatus::Stopped);
    assert!(stopped.queue.stopped_explicitly);
    std::fs::write(&released, "release").unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), delivery)
            .await
            .unwrap()
            .unwrap(),
        Tail::Return
    ));
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.queue.stopped_explicitly);
    assert_eq!(saved.attempts.len(), 1);
    assert_eq!(saved.cost_usd, 0.02);
}
