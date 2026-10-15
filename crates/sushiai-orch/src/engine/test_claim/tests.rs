#[cfg(test)]
use crate::task::Check;

#[cfg(test)]
use crate::engine::test_support::init_git_repo;
#[cfg(test)]
use crate::engine::test_support::task_with_status;
#[cfg(test)]
use crate::engine::test_support::test_app;
#[cfg(test)]
use crate::engine::test_support::test_app_fitting_sandbox;
use crate::harness::cancel::CancelToken;
use crate::task::Baseline;
use crate::task::PlannedTest;
use crate::task::Task;
use crate::task::TaskStatus;
use std::path::Path;
use std::path::PathBuf;

use super::checkout::{accept_rewrite, scratch_path};
#[cfg(test)]
mod cases {
    use super::*;

    #[test]
    fn the_judge_checkout_is_not_under_a_path_the_run_may_not_read() {
        let (app, _dir) = test_app();
        let repo = app.home.join("some-repo");
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().worktree =
            crate::git::worktree::worktree_path(&repo, &app.home, "", "task/x")
                .to_string_lossy()
                .to_string();
        let deny_read = PathBuf::from(&app.data_dir);
        for name in ["claim-tests", "claim-base"] {
            assert!(!scratch_path(&task, name).starts_with(&deny_read));
            assert_eq!(
                scratch_path(&task, name).parent(),
                Path::new(&task.worktree).parent()
            );
        }
        // The old place, inside the task's data dir, would have been denied.
        assert!(app
            .store
            .task_dir(&task.id)
            .join("runs/claim-tests")
            .starts_with(&deny_read));
    }

    /// A task gated by `sh t/a.test`, which fails on the base: the base has
    /// no `answer.txt`. `old` is the planner's test file as committed.
    fn gated_task(dir: &tempfile::TempDir, old: &str) -> Task {
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = dir.path().join("task").to_string_lossy().to_string();
        task.fixture_data().base_sha = crate::git::history::head_sha(&repo).unwrap();
        task.fixture_data().criteria = vec!["the answer is written".into()];
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "sh t/a.test".into(),
            baseline: Some(Baseline::Fail),
        }];
        task.fixture_data().planner_tests.files = vec![PlannedTest {
            path: "t/a.test".into(),
            content: old.into(),
        }];
        task
    }

    #[tokio::test]
    async fn a_shorter_rewrite_that_still_fails_on_the_base_is_accepted() {
        let (app, dir) = test_app_fitting_sandbox();
        let old = "#!/bin/sh\nset -e\n# the file must exist and hold the answer\ntest -f answer.txt\ntest -s answer.txt\ngrep -q 42 answer.txt\necho checked\n";
        let task = gated_task(&dir, old);
        // Well under half of the original's lines, and it fails on the base.
        let new = "test -f answer.txt\n".to_string();
        let files = accept_rewrite(&app, &task, 0, new.clone(), &CancelToken::new())
            .await
            .unwrap();
        assert_eq!(files[0].content, new);
    }

    #[tokio::test]
    async fn a_rewrite_that_passes_on_the_base_is_refused_whatever_its_length() {
        let (app, dir) = test_app_fitting_sandbox();
        let old = "#!/bin/sh\nset -e\n# the file must exist and hold the answer\ntest -f answer.txt\ntest -s answer.txt\ngrep -q 42 answer.txt\necho checked\n";
        let task = gated_task(&dir, old);
        // Same number of lines as the original, and it passes on the base.
        let same_length =
            "#!/bin/sh\nset -e\n# nothing is checked here\ntrue\ntrue\ntrue\necho checked\n";
        let shorter = "true\n";
        for new in [same_length, shorter] {
            let err = accept_rewrite(&app, &task, 0, new.to_string(), &CancelToken::new())
                .await
                .unwrap_err();
            assert!(err.contains("no longer fails on the base"), "{err}");
        }
    }
}
