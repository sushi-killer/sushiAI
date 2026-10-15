use super::*;
use crate::engine::test_support::{task_with_status, test_app};
use crate::task::AttemptStatus;

fn observed(name: &str) -> VerifyOutcome {
    VerifyOutcome {
        command: "synthetic-check".into(),
        code: Some(1),
        failing: vec![name.into()],
        tail: format!("test {name} failed: synthetic output"),
        ..Default::default()
    }
}

#[test]
fn failure_identity_ignores_passing_tests_and_preserves_numeric_test_names() {
    let mut first = observed("case_1");
    let id = cause(&first);
    first.passing.push("unrelated".into());
    first.tail.push_str(" 912 ms");
    assert_eq!(cause(&first), id);
    assert_ne!(cause(&observed("case_2")), id);
    first.failing = vec!["case_b".into(), "case_a".into()];
    let reversed = cause(&first);
    first.failing.reverse();
    assert_eq!(cause(&first), reversed);
}

#[test]
fn the_first_failure_queues_one_step_and_same_cause_after_that_step_waits_for_owner() {
    let (app, _dir) = test_app();
    let mut before = task_with_status(TaskStatus::Done);
    let failed = observed("named_test_7");
    app.store.save_task(&before).unwrap();
    let (queued, repair) = record_request(&app, &before, &failed, None, "new-base".into())
        .unwrap()
        .unwrap();
    assert!(repair);
    assert_eq!(queued.status, TaskStatus::Queued);
    assert!(queued.pr_pending);
    let step = queued.queue.pr_check_repair.as_ref().unwrap().step;
    before = queued;
    let mut attempt = crate::engine::test_support::attempt_with_failure(step, "old");
    attempt.n = step;
    attempt.status = AttemptStatus::Passed;
    before.fixture_data().attempts.push(attempt);
    before.resume_phase(TaskStatus::Done, now_ms());
    app.store.save_task(&before).unwrap();
    let (waiting, repair) = record_request(&app, &before, &failed, None, "new-base".into())
        .unwrap()
        .unwrap();
    assert!(!repair);
    assert_eq!(waiting.status, TaskStatus::Waiting);
    assert!(waiting.pr_pending);
    let question = waiting.question.as_ref().unwrap();
    assert_eq!(question.kind, QuestionKind::PrCheck);
    assert!(question.for_owner);
    assert!(question.text.contains("named_test_7") && question.text.contains("synthetic output"));
    assert_eq!(question.commands, [failed.command]);
}

#[test]
fn a_changed_contract_does_not_publish_an_old_repair_or_question() {
    let (app, _dir) = test_app();
    let before = task_with_status(TaskStatus::Done);
    let mut newer = before.clone();
    newer.fixture_data().criteria.push("new contract".into());
    app.store.save_task(&newer).unwrap();
    assert!(
        record_request(&app, &before, &observed("test"), None, "base".into())
            .unwrap()
            .is_none()
    );
    let saved = app.store.load_task(&before.id).unwrap().unwrap();
    assert_eq!(saved.criteria, ["new contract"]);
    assert!(saved.queue.pr_check_repair.is_none() && saved.question.is_none());
}

#[tokio::test]
async fn an_owner_overturn_before_repair_admission_defers_the_old_failure() {
    let (app, _dir) = test_app();
    let mut before = task_with_status(TaskStatus::Done);
    before.record_assumption(crate::task::Assumption {
        question: "Accept the inherited check?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "The base has the same named failure".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(crate::task::QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["check".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&before).unwrap();
    let id = before.id.clone();
    let worker_app = app.clone();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn(async move {
        blocked.await.unwrap();
        record_request(&worker_app, &before, &observed("case"), None, "base".into()).unwrap()
    });
    app.dispatch(
        "task.overturn",
        serde_json::json!({"id":id,"index":0,"answer":"Keep this check"}),
    )
    .await
    .unwrap();
    release.send(()).unwrap();
    assert!(
        worker.await.unwrap().is_none(),
        "a stale check scheduled a new repair"
    );
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert!(saved.assumptions[0].overturned);
    assert!(saved.queue.pr_check_repair.is_none());
    assert!(saved.question.is_none());
    assert_eq!(saved.status, TaskStatus::Done);
}

#[tokio::test]
async fn an_owner_overturn_during_repair_setup_keeps_a_live_task_loop() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = crate::policy::settings::SandboxMode::Host;
    let mut before = repair_setup_fixture(dir.path());
    before.fixture_data().queue.paused_until = Some(now_ms() + 60_000);
    app.store.save_task(&before).unwrap();
    let id = before.id.clone();
    let worker_app = app.clone();
    let worker = tokio::spawn(async move {
        resolve_checks(&worker_app, &before, "main", &observed("case"), None)
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !dir.path().join("setup-started").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the real repair setup did not reach its barrier");
    app.dispatch(
        "task.overturn",
        serde_json::json!({
            "id": id, "index": 0, "answer": "Keep this check"
        }),
    )
    .await
    .unwrap();
    std::fs::write(dir.path().join("setup-release"), "").unwrap();
    let result = worker.await.unwrap();
    let saved = app.store.load_task(&id).unwrap().unwrap();
    assert!(
        matches!(result, PrCarry::ChecksPending),
        "a current repair was silently deferred"
    );
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(saved.queue.pr_check_repair.is_some() && saved.pr_pending);
    assert!(saved.assumptions[0].overturned);
    assert!(
        app.controls.lock().unwrap().contains_key(&id),
        "the queued repair has no live loop"
    );
    app.dispatch("task.stop", serde_json::json!({"id":id}))
        .await
        .unwrap();
}

fn repair_setup_fixture(root: &std::path::Path) -> Task {
    use crate::engine::test_support::{git_in, init_git_repo};
    let repo = root.join("repair-repo");
    std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    let setup = serde_json::json!({"setup": format!(
        ": > '{}'; while [ ! -e '{}' ]; do sleep 0.01; done",
        root.join("setup-started").display(), root.join("setup-release").display()
    )});
    std::fs::write(repo.join(".sushiai/setup.json"), setup.to_string()).unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "repair setup fixture"]);
    git_in(&repo, &["branch", "task/repair"]);
    git_in(&repo, &["remote", "add", "origin", repo.to_str().unwrap()]);
    git_in(&repo, &["fetch", "-q", "origin", "main"]);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().worktree = root.join("repair-worktree").to_string_lossy().into_owned();
    task.fixture_data().branch = "task/repair".into();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    task.fixture_data().base_ref = Some("main".into());
    task.record_assumption(crate::task::Assumption {
        question: "Accept the inherited check?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "case fails on base".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["synthetic-check".into()],
        overturned: false,
        owner_answer: None,
    });
    task
}
