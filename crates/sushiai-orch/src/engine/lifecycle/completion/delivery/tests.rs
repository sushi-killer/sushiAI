use super::{landing_outcome, Tail};
use crate::engine::graph::landing::Landing;
use crate::engine::lifecycle::attempt_files::AttemptFiles;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::run_control::{TaskControl, TaskSignals};
use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::pipelines::delivery::{DeliveryFlow, DeliveryStep};
use crate::task::{AttemptStatus, FailureKind, Stage, Task, TaskStatus};
use std::path::Path;

fn ready_attempt(app: &App, repo: &Path) -> (Task, AttemptFiles) {
    std::fs::create_dir_all(repo).unwrap();
    init_git_repo(repo);
    let mut task = task_with_status(TaskStatus::Running);
    task.fixture_data().repo = repo.display().to_string();
    task.fixture_data().worktree = task.repo.clone();
    task.fixture_data().branch = git_in(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    task.fixture_data().base_sha = git_in(repo, &["rev-parse", "HEAD"]);
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
    app.store.save_task(&task).unwrap();
    std::fs::write(repo.join("README.md"), "completed work\n").unwrap();
    let files = AttemptFiles::new(index, 1, repo.into(), app.store.run_dir(&task.id, 1));
    (task, files)
}

#[test]
fn native_landing_receipts_route_to_their_typed_settlement() {
    let flow = DeliveryFlow::compiled();
    let cases = [
        (None, DeliveryStep::Commit),
        (Some(Landing::NoParent), DeliveryStep::Commit),
        (Some(Landing::DefaultBranchRefused), DeliveryStep::Commit),
        (Some(Landing::Waiting), DeliveryStep::Wait),
        (Some(Landing::Landed), DeliveryStep::Landed),
        (
            Some(Landing::Failed {
                kind: FailureKind::Error,
                detail: "conflict".into(),
            }),
            DeliveryStep::Retry,
        ),
        (Some(Landing::Rereview), DeliveryStep::Review),
        (Some(Landing::Cancelled), DeliveryStep::Cancel),
    ];
    for (receipt, settlement) in cases {
        assert_eq!(
            flow.next(DeliveryStep::Land, landing_outcome(&receipt)),
            settlement
        );
    }
}

#[tokio::test]
async fn an_owner_pending_amendment_rechecks_before_any_yaml_delivery_operation() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    let (mut task, files) = ready_attempt(&app, &repo);
    let head = git_in(&repo, &["rev-parse", "HEAD"]);
    let signals = TaskSignals::new();
    app.controls.lock().unwrap().insert(
        task.id.clone(),
        TaskControl {
            signals: signals.clone(),
            handle: tokio::spawn(async {}),
        },
    );
    let answer = app
        .dispatch(
            "task.amend",
            serde_json::json!({
                "id": task.id, "criteria": ["Owner's updated criterion"]
            }),
        )
        .await
        .unwrap();
    assert_eq!(answer["pending"], true);
    let expected = app
        .store
        .load_task(&task.id)
        .unwrap()
        .unwrap()
        .pending_amendment
        .clone();
    let mut budget = 1;
    let mut permit = None;
    let tail = files
        .finish(
            &app,
            &mut task,
            &mut budget,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
    assert!(matches!(tail, Tail::Continue));
    assert_eq!(budget, 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.pending_amendment, expected);
    assert_eq!(saved.attempts.len(), 1);
    assert_eq!(saved.attempts[0].status, AttemptStatus::Running);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.02));
    assert_eq!(saved.cost_usd, 0.02);
    assert!(saved.pr_url.is_none());
    assert_eq!(git_in(&repo, &["rev-parse", "HEAD"]), head);
}

#[tokio::test]
async fn a_persisted_owner_stop_keeps_the_native_receipt_and_never_commits() {
    let (app, dir) = test_app();
    let repo = dir.path().join("repo");
    let (mut task, files) = ready_attempt(&app, &repo);
    let head = git_in(&repo, &["rev-parse", "HEAD"]);
    task.fixture_data().status = TaskStatus::Stopped;
    task.fixture_data().queue.stopped_explicitly = true;
    app.store.save_task(&task).unwrap();
    let signals = TaskSignals::new();
    let mut budget = 1;
    let mut permit = None;
    let tail = files
        .finish(
            &app,
            &mut task,
            &mut budget,
            &mut QuestionSlot::new(&signals, &mut permit),
        )
        .await;
    assert!(matches!(tail, Tail::Return));
    assert_eq!(budget, 1);
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Stopped);
    assert!(saved.queue.stopped_explicitly);
    assert_eq!(saved.attempts[0].status, AttemptStatus::Passed);
    assert_eq!(saved.attempts[0].cost_usd, Some(0.02));
    assert_eq!(saved.cost_usd, 0.02);
    assert!(saved.pr_url.is_none());
    assert_eq!(git_in(&repo, &["rev-parse", "HEAD"]), head);
}
