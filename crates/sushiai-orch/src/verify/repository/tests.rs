use super::*;
use crate::task::{AttemptStatus, TaskStatus};

#[test]
fn a_build_step_before_a_planner_test_keeps_the_check_the_planners() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success());
    };
    git(&["init", "-q"]);
    std::fs::write(dir.path().join("README.md"), "x\n").unwrap();
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=t@e",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "init",
    ]);
    let mut task =
        crate::task::test_support::sample_task("t", TaskStatus::Running, AttemptStatus::Running);
    task.fixture_data().repo = dir.path().to_string_lossy().into();
    task.fixture_data().base_sha = "HEAD".into();
    let source = |command: &str| classify_check_source(&task, dir.path(), command, true);
    assert_eq!(
        source("cargo build -p app && node --test tests/new.test.cjs"),
        CheckSource::Planner
    );
    assert_eq!(source("cargo build -p app"), CheckSource::Repo);
    assert_eq!(
        source("cargo build -p app && node --test README.md"),
        CheckSource::Repo
    );
    assert_eq!(source("cargo build -p app && npm test"), CheckSource::Repo);
}
