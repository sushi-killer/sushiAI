use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::held_out::run_held_out_cached;
use crate::engine::verification::runner::run_checks;
use crate::engine::verification::runner::run_verify_commands;
use crate::engine::verification::runner::CheckRunner;
use crate::harness::cancel::CancelToken;
use crate::task::checks::types::Baseline;
use crate::task::checks::types::Check;
use crate::task::state::TaskStatus;
use crate::verify::passes::check_tree;
use crate::verify::passes::is_interrupted;
use std::time::Duration;

#[cfg(test)]
mod cases {
    use super::*;

    #[tokio::test]
    async fn a_timed_out_gate_runs_twice_then_runs_afresh() {
        let (app, dir) = test_app();
        *app.check_load.write().unwrap() = || (0.0, 4);
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then sleep 60; fi",
            counter.display(),
            counter.display()
        )];
        let cancel = CancelToken::new();
        let mut runner = CheckRunner::new(&app, &repo, None, "base", &cancel);
        runner.timeout = Duration::from_secs(2);
        let (results, lines) = runner.run(&commands).await;
        assert!(is_interrupted(&results[0]));
        assert_eq!(results[0].code, None);
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("was interrupted after")
                && lines[0].contains("; ran it again at once: interrupted after")
        );
        let (results, lines) = runner.run(&commands).await;
        assert_eq!(results[0].code, Some(0));
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
    }

    #[tokio::test]
    async fn a_tree_change_runs_the_command_again() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!("echo run >> '{}'", counter.display())];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                CheckRunner::new(&app, &repo, None, "base", &cancel),
                &commands,
            )
        };
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        let (results, lines) = run().await;
        assert_eq!(results[0].ms, 0);
        assert_eq!(lines.len(), 1);
        std::fs::write(repo.join("README.md"), "a different tree").unwrap();
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_check_that_changes_the_tree_cannot_supply_a_pass_for_either_tree() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; if [ \"$(cat README.md)\" = hello ]; then echo changed > README.md; fi",
            counter.display()
        )];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                CheckRunner::new(&app, &repo, None, "base", &cancel),
                &commands,
            )
        };
        let original = check_tree(&repo).await.unwrap();
        let (outcomes, _) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        let changed = check_tree(&repo).await.unwrap();
        assert_ne!(original, changed);
        for tree in [&original, &changed] {
            assert!(!app
                .gate_passes
                .lock()
                .unwrap()
                .contains(tree, &commands[0], "base"));
        }

        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        let (_, lines) = run().await;
        assert!(lines
            .iter()
            .any(|line| line.contains("already passed on tree")));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );

        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        let (_, lines) = run().await;
        assert!(lines.is_empty());
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
    }

    #[tokio::test]
    async fn a_failed_final_gate_retries_only_its_failed_command() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let passed = dir.path().join("passed");
        let failed = dir.path().join("failed");
        let ready = dir.path().join("ready");
        let commands = vec![
            format!("echo run >> '{}'", passed.display()),
            format!(
                "echo run >> '{}'; test -f '{}'",
                failed.display(),
                ready.display()
            ),
        ];
        let cancel = CancelToken::new();
        let run = || {
            run_verify_commands(
                crate::engine::verification::runner::CheckRunner::from_context(
                    CheckContext::new(&app, None).with_gate("final checks"),
                    &repo,
                    None,
                    "base",
                    &cancel,
                ),
                &commands,
            )
        };
        let first = run().await;
        assert_eq!(first[0].code, Some(0));
        assert_eq!(first[1].code, Some(1));
        std::fs::write(&ready, "").unwrap();
        let second = run().await;
        assert_eq!(second[0].ms, 0);
        assert!(second[0].decisions[0].contains("already passed on tree"));
        assert_eq!(second[1].code, Some(0));
        assert_eq!(std::fs::read_to_string(passed).unwrap().lines().count(), 1);
        assert_eq!(std::fs::read_to_string(failed).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn an_interrupted_check_records_the_tree_of_its_rerun() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; if [ \"$(cat README.md)\" = hello ]; then echo changed > README.md; exit 137; fi",
            counter.display()
        )];
        let cancel = CancelToken::new();
        let run = || {
            run_checks(
                CheckRunner::new(&app, &repo, None, "base", &cancel),
                &commands,
            )
        };
        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].code, Some(0));
        assert_eq!(lines.len(), 1);
        let (outcomes, lines) = run().await;
        assert_eq!(outcomes[0].ms, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("already passed on tree"));
        assert_eq!(std::fs::read_to_string(counter).unwrap().lines().count(), 2);
    }

    #[tokio::test]
    async fn a_held_out_decision_uses_its_label_and_hides_its_command() {
        let (app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let task = task_with_status(TaskStatus::Running);
        let cancel = CancelToken::new();
        let runner = CheckRunner::new(&app, &repo, None, &task.base_sha, &cancel);
        let mut check = Check {
            criterion: 2,
            run: "true".into(),
            baseline: Some(Baseline::Fail),
        };
        let (_, _, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert!(lines.is_empty());
        let (result, detail, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert_eq!(result.command, "held-out check (criterion 2)");
        assert_eq!(result.ms, 0);
        assert!(detail.is_none());
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .starts_with("Orchestrator: held-out check (criterion 2) already passed on tree "));
        assert!(!lines[0].contains(&check.run));
        check.run = "cat held.sh; exit 137".into();
        std::fs::write(repo.join("held.sh"), &check.run).unwrap();
        let (result, detail, lines) = run_held_out_cached(&runner, &task, &check).await;
        assert_eq!(result.code, Some(137));
        assert!(detail.is_some());
        assert!(!result.tail.contains(&check.run));
        assert_eq!(lines.len(), 1);
        assert!(lines[0]
            .starts_with("Orchestrator: held-out check (criterion 2) was interrupted after "));
        assert!(!lines[0].contains(&check.run));
        assert!(lines[0].contains("<held-out check>"));
    }
}
