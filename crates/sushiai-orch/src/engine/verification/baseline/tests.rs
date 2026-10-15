use crate::engine::test_support::git_in;
use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::test_support::test_app_fitting_sandbox;
use crate::engine::verification::baseline::baseline_checks;
use crate::engine::verification::baseline::gated_checks;
use crate::engine::verification::baseline::gated_held_out;
use crate::engine::verification::baseline::needs_baseline;
use crate::engine::verification::cache::run_verify_cached;
use crate::engine::verification::context::CheckContext;
use crate::engine::verification::held_out::run_held_out;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::SandboxMode;
use crate::policy::variant::Variant;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::baseline::baseline_todo;
use crate::task::checks::types::Baseline;
use crate::task::checks::types::Check;
use crate::task::state::TaskStatus;
use crate::verify::output::classify_baseline;
use crate::verify::output::timed_out_tail;
use crate::verify::output::CANCELLED_TAIL;
use crate::verify::output::INTERNAL_ERROR_PREFIX;
use crate::verify::output::SPAWN_FAILED_PREFIX;
use std::time::Duration;

#[cfg(test)]
mod grounded_checks_tests {
    use super::*;

    #[test]
    fn disabling_held_out_keeps_regular_baselines_and_gated_checks() {
        let mut task = task_with_status(TaskStatus::Running);
        let check = |run: &str| Check {
            criterion: 0,
            run: run.into(),
            baseline: None,
        };
        task.fixture_data().checks = vec![check("regular")];
        task.fixture_data().held_out = Some(check("hidden"));
        assert!(task.variant().held_out);
        assert_eq!(
            baseline_todo(&task),
            vec![(Some(0), 0, "regular".into()), (None, 0, "hidden".into())]
        );
        task.fixture_data().variant = Some(Variant {
            held_out: false,
            ..Variant::default()
        });
        assert!(needs_baseline(&task));
        assert_eq!(baseline_todo(&task), vec![(Some(0), 0, "regular".into())]);
        task.fixture_data().checks[0].baseline = Some(Baseline::Fail);
        assert!(!needs_baseline(&task));
        assert_eq!(gated_checks(&task), task.checks);
        task.fixture_data().held_out.as_mut().unwrap().baseline = Some(Baseline::Fail);
        assert!(gated_held_out(&task).is_none());
        task.fixture_data().variant.as_mut().unwrap().held_out = true;
        assert_eq!(gated_held_out(&task), task.held_out);
    }

    fn outcome(code: Option<i32>, tail: &str) -> VerifyOutcome {
        VerifyOutcome {
            command: "c".into(),
            code,
            tail: tail.into(),
            ms: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_interrupted_verify_result_is_not_cached() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base = git_in(&repo, &["rev-parse", "HEAD"]);
        let counter = dir.path().join("counter");
        let commands = vec![format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then exit 143; fi",
            counter.display(),
            counter.display()
        )];
        let cancel = CancelToken::new();
        let repo_name = repo.to_string_lossy().into_owned();
        let run = || {
            run_verify_cached(
                &app,
                "task",
                &repo_name,
                &repo,
                dir.path(),
                &base,
                &commands,
                &cancel,
            )
        };
        let (first, lines) = run().await;
        assert_eq!(first[0].code, Some(143));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert!(app.verify_cache.lock().unwrap().is_empty());
        assert!(lines
            .iter()
            .any(|l| l.contains("was interrupted") && l.contains("interrupted after")));
        let (second, _) = run().await;
        assert_eq!(second[0].code, Some(0));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
        let (third, lines) = run().await;
        assert_eq!(third[0].code, Some(0));
        assert_eq!(third[0].ms, 0);
        assert!(lines.iter().any(|l| l.contains("already passed on tree")));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
    }

    #[test]
    fn a_baseline_is_classified_from_the_exit_code_and_the_tail() {
        assert_eq!(
            classify_baseline(&outcome(Some(0), "")),
            Some(Baseline::Pass)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(1), "")),
            Some(Baseline::Fail)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(126), "")),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(Some(127), "")),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(
                None,
                &format!("{SPAWN_FAILED_PREFIX}no such file")
            )),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(None, &format!("{INTERNAL_ERROR_PREFIX}pipe"))),
            Some(Baseline::Env)
        );
        assert_eq!(
            classify_baseline(&outcome(None, &timed_out_tail(Duration::from_secs(1200)))),
            Some(Baseline::Fail)
        );
        for code in [124, 137, 143] {
            assert_eq!(
                classify_baseline(&outcome(Some(code), "")),
                Some(Baseline::Fail)
            );
        }
        assert_eq!(classify_baseline(&outcome(None, CANCELLED_TAIL)), None);
    }

    #[tokio::test]
    async fn the_held_out_command_is_scrubbed_from_the_outcome_and_the_detail() {
        let dir = tempfile::tempdir().unwrap();
        // The command prints itself, so its output would leak it.
        let command = "cat held.sh; false";
        std::fs::write(dir.path().join("held.sh"), command).unwrap();
        let task = crate::task::test_support::fixture(crate::task::state::TaskData {
            criteria: vec!["the first".into(), "the second".into()],
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let check = Check {
            criterion: 1,
            run: command.into(),
            baseline: Some(Baseline::Fail),
        };
        let (app, _app_dir) = test_app();
        let (result, detail) = run_held_out(
            crate::engine::verification::runner::CheckRunner::from_context(
                CheckContext::new(&app, None),
                dir.path(),
                None,
                &task.base_sha,
                &CancelToken::new(),
            ),
            &task,
            &check,
        )
        .await;
        assert_eq!(result.command, "held-out check (criterion 1)");
        assert_eq!(result.code, Some(1));
        let detail = detail.unwrap();
        assert!(
            detail.starts_with("Held-out check for criterion 1 failed: the second. exited 1.\n"),
            "{detail}"
        );
        assert!(detail.contains("<held-out check>"), "{detail}");
        assert!(!detail.contains(command) && !result.tail.contains(command));
    }

    #[tokio::test]
    async fn a_baseline_undoes_what_the_checks_did_to_the_worktree() {
        let repo = tempfile::tempdir().unwrap();
        let sh = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success());
        };
        sh(&["init", "-q"]);
        sh(&["config", "user.email", "t@example.com"]);
        sh(&["config", "user.name", "t"]);
        std::fs::write(repo.path().join("tracked.txt"), "one\n").unwrap();
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "init"]);
        let (app, _dir) = test_app_fitting_sandbox();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().criteria = vec!["a".into(), "b".into(), "c".into()];
        let check = |criterion, run: &str| Check {
            criterion,
            run: run.into(),
            baseline: None,
        };
        task.fixture_data().checks = vec![
            check(
                0,
                "echo x > made.txt; mkdir -p d/e; echo y > d/e/f.txt; echo more >> tracked.txt; false",
            ),
            check(1, "true"),
            check(2, "no-such-command-anywhere"),
        ];
        let before = crate::git::changes::status_entries(repo.path()).unwrap();
        assert!(
            baseline_checks(
                &app,
                &task.id.clone(),
                &mut task,
                repo.path(),
                &CancelToken::new()
            )
            .await
        );
        assert_eq!(
            crate::git::changes::status_entries(repo.path()).unwrap(),
            before
        );
        assert!(!repo.path().join("made.txt").exists() && !repo.path().join("d").exists());
        assert_eq!(
            std::fs::read_to_string(repo.path().join("tracked.txt")).unwrap(),
            "one\n"
        );
        let baselines: Vec<_> = task.checks.iter().map(|c| c.baseline).collect();
        assert_eq!(
            baselines,
            vec![
                Some(Baseline::Fail),
                Some(Baseline::Pass),
                Some(Baseline::Env)
            ]
        );
        assert!(task.decisions.contains(
            &"Orchestrator: check for criterion 1 already passes on base, not grounded: true"
                .to_string()
        ));
        assert!(task.decisions.iter().any(|d| d.starts_with(
            "Orchestrator: check for criterion 2 could not run on base (exit 127), left out: no-such"
        )));
        assert!(!needs_baseline(&task));
    }
}
