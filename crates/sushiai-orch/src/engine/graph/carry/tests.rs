use super::*;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{TaskStatus, VerifyOutcome};

fn parsed(command: &str, text: &str, code: i32) -> VerifyOutcome {
    let report = crate::policy::test_output::parse(text, "");
    VerifyOutcome {
        command: command.into(),
        code: Some(code),
        tail: text.into(),
        failing: report.failing,
        passing: report.passing,
        output_key: report.rest_key,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_flaky_carried_final_reruns_in_the_carried_checkout_and_records_named_waiver() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let carried = dir.path().join("carried");
    let original = dir.path().join("original");
    std::fs::create_dir_all(&carried).unwrap();
    std::fs::create_dir_all(&original).unwrap();
    init_git_repo(&carried);
    std::fs::write(carried.join("ci.sh"), "if [ ! -e .observed ]; then touch .observed; printf 'not ok 1 - flaky_case_7\\n'; exit 1; fi\nprintf 'ok 1 - flaky_case_7\\n'\n").unwrap();
    std::fs::write(carried.join(".gitignore"), ".observed\n").unwrap();
    git_in(&carried, &["add", "."]);
    git_in(&carried, &["commit", "-qm", "fixture"]);
    std::fs::write(original.join("ci.sh"), "touch wrong-checkout; exit 1\n").unwrap();
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = carried.to_string_lossy().into_owned();
    task.fixture_data().worktree = original.to_string_lossy().into_owned();
    task.fixture_data().base_sha = git_in(&carried, &["rev-parse", "HEAD"]);
    let command = "sh ci.sh".to_string();
    app.base_runs.lock().unwrap().insert(
        (task.base_sha.clone(), command.clone()),
        parsed(&command, "not ok 1 - baseline_case\n", 1),
    );
    let cancel = CancelToken::new();
    let run_dir = dir.path().join("runs");
    let result = CarryTree::new(&carried, &run_dir, None, &cancel)
        .verify(&app, &mut task, std::slice::from_ref(&command), true)
        .await;
    assert!(result.is_ok());
    assert!(!original.join("wrong-checkout").exists());
    assert!(carried.join(".observed").exists());
    assert_eq!(task.delivery_checks.len(), 1);
    assert_eq!(task.delivery_checks[0].code, Some(1));
    assert_eq!(task.delivery_checks[0].failing, ["flaky_case_7"]);
    assert!(
        task.attempts.is_empty(),
        "delivery observations never invent an attempt"
    );
    assert!(task
        .decisions
        .iter()
        .any(|line| line.contains("flaky_case_7") && line.contains("reruns")));
    assert!(task
        .assumptions
        .iter()
        .any(|waiver| waiver.commands == [command.clone()]
            && waiver.evidence.contains("flaky_case_7")));
}

#[tokio::test]
async fn a_persistent_carried_final_keeps_the_actual_failure_after_one_rerun() {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    std::fs::write(
        repo.join("ci.sh"),
        "echo run >> .runs; printf 'not ok 1 - persistent_case_9\\n'; exit 1\n",
    )
    .unwrap();
    std::fs::write(repo.join(".gitignore"), ".runs\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "fixture"]);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().worktree = task.repo.clone();
    task.fixture_data().base_sha = git_in(&repo, &["rev-parse", "HEAD"]);
    let command = "sh ci.sh".to_string();
    app.base_runs.lock().unwrap().insert(
        (task.base_sha.clone(), command.clone()),
        parsed(&command, "ok 1 - persistent_case_9\n", 0),
    );
    let cancel = CancelToken::new();
    let run_dir = dir.path().join("runs");
    let result = CarryTree::new(&repo, &run_dir, None, &cancel)
        .verify(&app, &mut task, &[command], true)
        .await;
    let Err(CarryFailure::Check(failed)) = result else {
        panic!("persistent check must stay typed")
    };
    assert_eq!(failed.failing, ["persistent_case_9"]);
    assert!(failed.tail.contains("not ok 1 - persistent_case_9"));
    assert_eq!(
        std::fs::read_to_string(repo.join(".runs"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert!(task.assumptions.is_empty());
}
