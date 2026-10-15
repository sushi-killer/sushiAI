use super::*;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{AskedBy, Question, QuestionKind, Task, TaskStatus};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

struct Release(PathBuf);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

fn waiting() -> Task {
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().criteria = vec!["The owner check passes".into()];
    task.fixture_data().held_out = Some(crate::task::Check {
        criterion: 0,
        run: "owner-check".into(),
        baseline: Some(crate::task::Baseline::Fail),
    });
    task.ask(
        Question::new(
            "Choose",
            vec!["continue".into()],
            QuestionKind::AgentQuestion,
            AskedBy::Implement,
            1,
        ),
        1,
    );
    task
}

#[tokio::test]
async fn an_idle_waiting_amendment_returns_before_its_unbounded_baseline_command_runs() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let entered = dir.path().join("entered");
    let released = dir.path().join("released");
    let _release = Release(released.clone());
    let command = format!(
        "touch '{}'; while ! test -f '{}'; do sleep 0.01; done; exit 1",
        entered.display(),
        released.display()
    );
    let mut task = waiting();
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    app.store.save_task(&task).unwrap();
    let answer = tokio::time::timeout(
        Duration::from_millis(500),
        app.dispatch(
            "task.amend",
            json!({"id":task.id,"checks":[{"criterion":0,"run":command}]}),
        ),
    )
    .await
    .expect("waiting amendments must not await baseline IO")
    .unwrap();
    assert_eq!(answer["pending"], false);
    assert!(!entered.exists());
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Waiting);
    assert_eq!(saved.checks[0].baseline, None);
    assert_eq!(saved.checks[0].run, command);
}

#[tokio::test]
async fn an_acknowledged_live_wait_edit_survives_the_daemon_and_applies_after_restart() {
    let (app, _dir) = test_app();
    let mut task = waiting();
    app.store.save_task(&task).unwrap();
    let id = task.id.clone();
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let waiting = tokio::spawn({
        let app = app.clone();
        let signals = signals.clone();
        async move {
            QuestionSlot::new(&signals, &mut None)
                .park(&app, &mut task)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while signals.pending_answer.lock().unwrap().is_none() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let answer = tokio::time::timeout(
        Duration::from_millis(500),
        app.dispatch(
            "task.amend",
            json!({"id":id,"criteria":["Owner's durable edit"],"heldOut":null}),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(answer["pending"], true);
    let stored = app.store.load_task(&id).unwrap().unwrap();
    assert_eq!(
        stored.pending_amendment,
        *signals.pending_amend.lock().unwrap()
    );
    assert!(stored.pending_amendment.is_some());
    let data = app.data_dir.clone();
    let home = app.home.clone();
    signals.cancel.cancel();
    assert_eq!(waiting.await.unwrap(), None);
    app.controls.lock().unwrap().remove(&id);
    drop(app);
    let restarted = App::new(data, home, "orchd".into()).unwrap();
    restarted.recover_pending_amendments().unwrap();
    let saved = restarted.store.load_task(&id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["Owner's durable edit"]);
    assert!(saved.held_out.is_none());
    assert!(saved.pending_amendment.is_none());
    assert!(saved.question_history.is_empty());
    restarted.recover_pending_amendments().unwrap();
    let again = restarted.store.load_task(&id).unwrap().unwrap();
    assert_eq!(
        again
            .decisions
            .iter()
            .filter(|line| line.starts_with("Amended:"))
            .count(),
        1
    );
}
