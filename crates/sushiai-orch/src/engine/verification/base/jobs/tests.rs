use super::run_base_job;
use crate::engine::test_support::git_in;
use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::git::paths::short_sha;
use crate::policy::settings::SandboxMode;
use crate::task::state::TaskStatus;

#[tokio::test]
async fn a_flaky_background_base_check_records_its_decision_after_the_task_finishes() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_str().unwrap().into();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    let counter = dir.path().join("counter");
    let command = format!(
        "echo run >> '{}'; test \"$(wc -l < '{}')\" -ge 2",
        counter.display(),
        counter.display()
    );
    app.store.save_task(&task).unwrap();

    run_base_job(
        &app,
        crate::engine::verification::base::jobs::BaseCheckJob::new(
            Some(&task.id),
            &task.repo,
            &task.base_sha,
            &dir.path().join("runs"),
            true,
        ),
        std::slice::from_ref(&command),
    )
    .await;

    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    assert_eq!(saved.status, TaskStatus::Done);
    assert!(saved.decisions.contains(&format!(
        "Orchestrator: base check {command} flaky on {}: passed on rerun",
        short_sha(&task.base_sha)
    )));
    assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
    assert_eq!(
        app.base_runs.lock().unwrap()[&(task.base_sha.clone(), command)].code,
        Some(0)
    );
}
