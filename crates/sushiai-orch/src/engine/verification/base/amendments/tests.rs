use super::apply_pending_amendment;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{Baseline, Stage, Task, TaskStatus};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

fn repository_task(
    app: &crate::engine::state::App,
    dir: &tempfile::TempDir,
    status: TaskStatus,
) -> Task {
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut task = task_with_status(status);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    task.fixture_data().criteria = vec!["original contract".into()];
    task.fixture_data().verify = vec!["original-command".into()];
    app.store.save_task(&task).unwrap();
    task
}

#[tokio::test]
async fn a_non_owner_cannot_remove_a_shared_text_check_by_mentioning_its_command() {
    let (app, _dir) = test_app();
    let mut task = task_with_status(TaskStatus::Stopped);
    task.fixture_data().criteria = vec!["The repository passes -- check: npm run ci".into()];
    app.store.save_task(&task).unwrap();
    let before = serde_json::to_value(&task).unwrap();
    let result = app
        .dispatch(
            "task.amend",
            json!({
                "id": task.id,
                "source": "mcp",
                "criteria": ["The earlier npm run ci requirement is only a historical note"]
            }),
        )
        .await;
    assert!(
        result.is_err(),
        "a textual mention is not an executable check: {result:?}"
    );
    let after = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(serde_json::to_value(after).unwrap(), before);
}

#[tokio::test]
async fn retiring_a_loop_keeps_both_owner_edits_accepted_around_live_amendment_baselining() {
    let (app, dir) = test_app();
    let original = repository_task(&app, &dir, TaskStatus::Running);
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        original.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let entered = dir.path().join("baseline-entered");
    let released = dir.path().join("baseline-released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; exit 1",
        entered.display(),
        released.display()
    );
    let first = app
        .dispatch(
            "task.amend",
            json!({
                "id": original.id, "criteria": ["first owner contract"],
                "verify": ["first-owner-command"], "checks": [{"criterion": 0, "run": command}]
            }),
        )
        .await
        .unwrap();
    assert_eq!(first["pending"], true);
    let running_app = app.clone();
    let running_signals = signals.clone();
    let running = tokio::spawn(async move {
        let mut task = original;
        apply_pending_amendment(
            &running_app,
            &mut task,
            &running_signals.pending_amend,
            &running_signals.cancel,
        )
        .await;
        task
    });
    let reached = tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if reached.is_err() {
        std::fs::write(&released, "release").unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), running).await;
        panic!("live amendment baseline did not reach its barrier: {result:?}");
    }
    let id = app.store.list_tasks().unwrap()[0].id.clone();
    let second = tokio::time::timeout(Duration::from_secs(5), app.dispatch("task.amend", json!({
        "id": id, "verify": ["second-owner-current-command"], "finalVerify": ["second-owner-final-command"]
    }))).await.expect("the first baseline must not hold the task lock").unwrap();
    assert_eq!(second["pending"], true);
    std::fs::write(&released, "release").unwrap();
    let _task = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .unwrap()
        .unwrap();
    app.finish_task_loop(&id);
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["first owner contract"]);
    assert_eq!(saved.verify, ["second-owner-current-command"]);
    assert_eq!(saved.final_verify, ["second-owner-final-command"]);
    assert_eq!(saved.checks[0].run, command);
    assert_eq!(saved.checks[0].baseline, Some(Baseline::Fail));
    assert!(signals.pending_amend.lock().unwrap().is_none());
    assert!(!saved
        .decisions
        .iter()
        .any(|line| line.starts_with("Amendment not applied:")));
    assert!(!app.controls.lock().unwrap().contains_key(&id));
}

#[tokio::test]
async fn an_idle_amendment_that_meets_a_new_loop_is_queued_without_overwriting_its_attempt() {
    let (app, dir) = test_app();
    let original = repository_task(&app, &dir, TaskStatus::Stopped);
    let entered = dir.path().join("idle-baseline-entered");
    let released = dir.path().join("idle-baseline-released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; exit 1",
        entered.display(),
        released.display()
    );
    let first_app = app.clone();
    let id = original.id.clone();
    let running_id = id.clone();
    let amendment = tokio::spawn(async move {
        first_app.dispatch("task.amend", json!({
            "id": running_id, "criteria": ["accepted idle owner contract"],
            "verify": ["accepted-owner-command"], "checks": [{"criterion": 0, "run": command}]
        })).await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the idle amendment baseline must reach its barrier");
    let signals = TaskSignals::new();
    {
        let lock = app.task_lock(&id);
        let _guard = lock.lock().unwrap();
        let mut fresh = app.store.load_task(&id).unwrap().unwrap();
        fresh.resume_phase(TaskStatus::Running, 7);
        let route = app.settings.read().unwrap().routes[0].clone();
        crate::task::attempts::start(&mut fresh, Stage::Implement, &route, "new loop", 7);
        crate::task::attempts::observe_run(
            &mut fresh,
            1,
            crate::task::attempts::RunObservation::Session("new-active-session".into()),
            7,
        );
        app.store.save_task(&fresh).unwrap();
        app.controls.lock().unwrap().insert(
            id.clone(),
            TaskControl {
                signals: signals.clone(),
                handle: tokio::spawn(std::future::pending()),
            },
        );
    }
    std::fs::write(&released, "release").unwrap();
    let response = tokio::time::timeout(Duration::from_secs(10), amendment)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(response["pending"], true, "{response}");
    let mut saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.criteria, original.criteria);
    assert_eq!(saved.verify, original.verify);
    assert_eq!(saved.attempts.len(), 1);
    assert_eq!(
        saved.attempts[0].session_id.as_deref(),
        Some("new-active-session")
    );
    assert!(signals.pending_amend.lock().unwrap().is_some());
    apply_pending_amendment(&app, &mut saved, &signals.pending_amend, &signals.cancel).await;
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["accepted idle owner contract"]);
    assert_eq!(saved.verify, ["accepted-owner-command"]);
    assert_eq!(saved.checks[0].baseline, Some(Baseline::Fail));
    assert_eq!(
        saved.attempts[0].session_id.as_deref(),
        Some("new-active-session")
    );
    let control = app.controls.lock().unwrap().remove(&id).unwrap();
    control.handle.abort();
    assert!(control.handle.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn a_screenshot_only_owner_edit_invalidates_an_older_idle_baseline_publication() {
    let (app, dir) = test_app();
    let original = repository_task(&app, &dir, TaskStatus::Stopped);
    let entered = dir.path().join("screenshot-baseline-entered");
    let released = dir.path().join("screenshot-baseline-released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; exit 1",
        entered.display(),
        released.display()
    );
    let first_app = app.clone();
    let id = original.id.clone();
    let running_id = id.clone();
    let amendment = tokio::spawn(async move {
        first_app
            .dispatch(
                "task.amend",
                json!({
                    "id": running_id, "criteria": ["stale owner contract"],
                    "checks": [{"criterion": 0, "run": command}]
                }),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first owner baseline must reach its barrier");
    let response = app
        .dispatch(
            "task.amend",
            json!({
                "id": id, "screenshot": "capture-current-owner-evidence"
            }),
        )
        .await
        .unwrap();
    assert_eq!(response["pending"], false);
    let current = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(
        current.brief_check.screenshot.as_deref(),
        Some("capture-current-owner-evidence")
    );
    let before = serde_json::to_value(&current).unwrap();
    std::fs::write(&released, "release").unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), amendment)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("stale"), "{refused}");
    let after = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(serde_json::to_value(after).unwrap(), before);
}
