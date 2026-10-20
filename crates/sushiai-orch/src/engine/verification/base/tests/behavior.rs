use crate::clock::now_ms;
use crate::engine::answer_policy::record_answered_question;
use crate::engine::questions::parking::QuestionSlot;
use crate::engine::questions::reply::Reply;
use crate::engine::run_control::TaskSignals;
use crate::engine::test_support::git_in;
use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::verification::base::answers::base_check_answered;
use crate::engine::verification::base::answers::base_retry_requested;
use crate::engine::verification::base::answers::command_named_by_task;
use crate::engine::verification::base::answers::is_kept;
use crate::engine::verification::base::answers::is_task_target;
use crate::engine::verification::base::answers::keep_check;
use crate::engine::verification::base::answers::keep_planned_checks;
use crate::engine::verification::base::answers::non_owner_base_retry;
use crate::engine::verification::base::answers::record_base_answer;
use crate::engine::verification::base::answers::record_owner_base_check;
use crate::engine::verification::base::answers::KEPT_BY_TASK_TARGET;
use crate::engine::verification::base::answers::PRE_EXISTING_DROP;
use crate::engine::verification::base::checks::base_result;
use crate::engine::verification::base::checks::failing_twice_on_base;
use crate::engine::verification::base::questions::pre_existing_question;
use crate::engine::verification::base::run::run_on_base;
use crate::engine::verification::base::wait::BaseBreak;
use crate::engine::verification::base::wait::BaseFailure;
use crate::harness::cancel::CancelToken;
use crate::policy::settings::SandboxMode;
use crate::task::amendment::Amendment;
use crate::task::attempt::AttemptStatus;
use crate::task::attempt::VerifyOutcome;
use crate::task::checks::authority::Origin;
use crate::task::checks::commands::check_targets;
use crate::task::checks::types::Baseline;
use crate::task::checks::types::Check;
use crate::task::lifecycle::is_option;
use crate::task::lifecycle::PRE_EXISTING_RETRY;
use crate::task::questions::AnsweredBy;
use crate::task::state::TaskStatus;
use crate::verify::base_cache::is_environment_result;
use crate::verify::base_cache::load_base_runs;
use std::time::Duration;
use tokio::sync::watch;

#[cfg(test)]
mod cases {
    use super::*;

    fn outcome(command: &str, code: i32) -> VerifyOutcome {
        VerifyOutcome {
            command: command.to_string(),
            code: Some(code),
            tail: String::new(),
            ms: 1,
            ..Default::default()
        }
    }

    #[test]
    fn only_a_failed_setup_is_an_environment_result_never_a_tail() {
        let mut eexist = outcome("cargo test", 1);
        eexist.tail = "Error: EEXIST: file already exists, mkdir 'target'".into();
        let mut install = outcome("npm test", 1);
        install.tail = "setup failed (exit 1): npm ERR! failed to install".into();
        assert!(!is_environment_result(&eexist));
        assert!(!is_environment_result(&install));
        install.setup_failed = true;
        assert!(is_environment_result(&install));
    }

    #[tokio::test]
    async fn an_interrupted_base_run_is_not_cached() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base = git_in(&repo, &["rev-parse", "HEAD"]);
        let counter = dir.path().join("counter");
        let command = format!(
            "echo run >> '{}'; n=$(wc -l < '{}'); if [ \"$n\" -le 2 ]; then exit 137; fi",
            counter.display(),
            counter.display()
        );
        let commands = vec![command.clone()];
        let cancel = CancelToken::new();
        let run_dir = dir.path().join("runs");
        let first = run_on_base(
            &app,
            crate::engine::verification::base::jobs::BaseCheckJob::new(
                None,
                repo.to_str().unwrap(),
                &base,
                &run_dir,
                false,
            ),
            &commands,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(first[0].code, Some(137));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            2
        );
        assert!(app.base_runs.lock().unwrap().is_empty());
        assert!(load_base_runs(&app.data_dir).is_empty());
        let second = run_on_base(
            &app,
            crate::engine::verification::base::jobs::BaseCheckJob::new(
                None,
                repo.to_str().unwrap(),
                &base,
                &run_dir,
                false,
            ),
            &commands,
            &cancel,
        )
        .await
        .unwrap();
        assert_eq!(second[0].code, Some(0));
        assert_eq!(
            std::fs::read_to_string(&counter).unwrap().lines().count(),
            3
        );
        assert_eq!(app.base_runs.lock().unwrap().len(), 1);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_str().unwrap().into();
        task.fixture_data().base_sha = base.clone();
        assert!(failing_twice_on_base(&app, &mut task, &commands, &cancel)
            .await
            .is_empty());
        assert!(task
            .decisions
            .iter()
            .any(|l| l.contains("was interrupted") && l.contains("after") && l.contains(" s")));
    }

    #[tokio::test]
    async fn a_pair_in_flight_is_waited_for_and_never_started_twice() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let key = ("abc".to_string(), "npm run ci".to_string());
        let (tx, rx) = watch::channel(false);
        app.base_jobs.lock().unwrap().insert(key.clone(), rx);
        let cancel = CancelToken::new();
        let waiter = {
            let app = app.clone();
            let run_dir = app.data_dir.join("runs");
            tokio::spawn(async move {
                base_result(
                    &app,
                    crate::engine::verification::base::jobs::BaseCheckJob::new(
                        None,
                        "/no/such/repo",
                        "abc",
                        &run_dir,
                        true,
                    ),
                    "npm run ci",
                    &cancel,
                )
                .await
            })
        };
        crate::engine::test_support::holds_for_async(
            "the waiter waits for the one job",
            Duration::from_millis(100),
            || !waiter.is_finished() && app.base_jobs.lock().unwrap().len() == 1,
        )
        .await;
        // The job ends: its result is cached first, then it leaves the registry.
        app.base_runs
            .lock()
            .unwrap()
            .insert(key.clone(), outcome("npm run ci", 1));
        app.base_jobs.lock().unwrap().remove(&key);
        tx.send(true).unwrap();
        let got = waiter.await.unwrap().expect("the job's result");
        assert_eq!(got.code, Some(1));
        assert!(app.base_jobs.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_job_caches_nothing_and_its_waiters_get_base_unknown() {
        let (app, dir) = crate::engine::test_support::test_app();
        app.base_cancel.cancel();
        let run_dir = dir.path().join("runs");
        let got = base_result(
            &app,
            crate::engine::verification::base::jobs::BaseCheckJob::new(
                None,
                "/no/such/repo",
                "abc",
                &run_dir,
                true,
            ),
            "npm run ci",
            &CancelToken::new(),
        )
        .await;
        assert!(got.is_none());
        assert!(app.base_runs.lock().unwrap().is_empty());
        assert!(app.base_jobs.lock().unwrap().is_empty());
        let left = std::fs::read_dir(dir.path().join("data/base-runs")).map_or(0, |d| d.count());
        assert_eq!(left, 0, "no scratch worktree is left");
    }

    #[tokio::test]
    async fn base_check_pairs_of_one_repo_run_one_at_a_time() {
        let (app, dir) = crate::engine::test_support::test_app();
        let task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let lock = app
            .base_check_locks
            .lock()
            .unwrap()
            .entry(task.repo.clone())
            .or_default()
            .clone();
        let _busy = lock.lock().await;
        let cancel = CancelToken::new();
        // Another pair of the same repo is running: this one waits for it
        // (here until it is cancelled) instead of running beside it.
        let mut task = task;
        let commands = ["true".to_string()];
        let call = failing_twice_on_base(&app, &mut task, &commands, &cancel);
        tokio::pin!(call);
        // timing: an absence window; a call that ignored the held lock would finish inside it
        let early = tokio::time::timeout(Duration::from_millis(150), &mut call).await;
        assert!(early.is_err(), "it ran beside the lock holder");
        cancel.cancel();
        let failed = call.await;
        assert!(failed.is_empty());
        assert!(
            !dir.path().join("data/base-runs").exists(),
            "it never started"
        );
    }

    #[tokio::test]
    async fn a_refused_drop_leaves_the_check_and_the_failed_attempt_and_retries() {
        let (app, dir) = crate::engine::test_support::test_app();
        app.settings.write().unwrap().answer_policy = false;
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        crate::engine::test_support::init_git_repo(&repo);
        let wt = dir.path().join("worktree");
        let created =
            crate::git::worktree::create_worktree(&repo, &task.branch, &wt, "HEAD").unwrap();
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(crate::git::branches::branch_of(&repo, "HEAD"));
        crate::engine::test_support::git_in(
            &repo,
            &["commit", "-q", "--allow-empty", "-m", "fix base"],
        );
        let new_base = crate::git::history::head_sha(&repo).unwrap();
        task.fixture_data().verify = vec!["npm run ci".into()];
        task.fixture_data().attempts =
            vec![crate::engine::test_support::attempt_with_failure(1, "x")];
        task.fixture_data().attempts[0].status = AttemptStatus::Failed;
        app.store.save_task(&task).unwrap();
        let id = task.id.clone();
        let failed = VerifyOutcome {
            command: "npm run ci".into(),
            code: Some(1),
            tail: "boom".into(),
            ms: 1,
            ..Default::default()
        };
        let signals = TaskSignals::new();
        let pending = signals.pending_answer.clone();
        let mut permit = None;
        // An agent's "drop all" that got past the RPC gate (the
        // setting changed in between, say) is still refused by the loop.
        let deliver = async {
            let mut asked = None;
            crate::engine::test_support::wait_async("the question to be asked", || {
                asked = pending.lock().unwrap().take();
                asked.is_some()
            })
            .await;
            app.agent_answers
                .lock()
                .unwrap()
                .insert(id.clone(), AnsweredBy::Orchestrator);
            asked
                .unwrap()
                .send(Reply {
                    text: PRE_EXISTING_DROP.to_string(),
                    default_evidence: None,
                    like: None,
                })
                .ok();
        };
        let (end, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            let mut slot = QuestionSlot::new(&signals, &mut permit);
            let failure = BaseFailure::new(0, &failed, &failed);
            tokio::join!(failure.wait(&app, &mut task, &mut slot), deliver)
        })
        .await
        .expect("a refused drop must retry on the moved base without another owner answer");
        assert!(matches!(end, BaseBreak::Retry));
        assert_eq!(task.base_sha, new_base);
        assert_eq!(crate::git::history::head_sha(&wt).unwrap(), new_base);
        assert!(task.question.is_none());
        assert_eq!(task.verify, vec!["npm run ci".to_string()]);
        assert_eq!(task.attempts[0].status, AttemptStatus::Failed);
        assert!(task.attempts[0].failure.is_some());
        assert_ne!(task.status, TaskStatus::Running);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("check npm run ci stays")));
    }

    #[test]
    fn a_failing_final_whose_missing_target_the_plan_creates_is_kept_by_rule() {
        let (app, _dir) = crate::engine::test_support::test_app();
        let repo = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(repo.path())
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        std::fs::write(repo.path().join("a.txt"), "a").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        let sha = git(&["rev-parse", "HEAD"]);
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.path().to_string_lossy().to_string();
        task.fixture_data().base_sha = sha;
        task.record_paths(vec!["tests/new_check.sh".to_string()]);
        let failing = vec![
            "sh tests/new_check.sh".to_string(),
            "sh tests/other.sh".to_string(),
        ];
        let kept = keep_planned_checks(&app, &mut task, &failing);
        assert_eq!(kept, vec!["sh tests/new_check.sh".to_string()]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("sh tests/new_check.sh")));
    }

    #[test]
    fn options_match_ignoring_case_space_and_trailing_punctuation() {
        assert!(is_option("Drop all.", PRE_EXISTING_DROP));
        assert!(is_option("  DROP ALL! ", PRE_EXISTING_DROP));
        assert!(is_option("drop all", PRE_EXISTING_DROP));
        assert!(is_option("Stop?!", "stop"));
        assert!(!is_option("drop all please", PRE_EXISTING_DROP));
        assert!(!is_option("keep all", PRE_EXISTING_DROP));
    }

    #[test]
    fn an_answered_pair_is_found_by_command_and_short_base_sha() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        assert!(!base_check_answered(&task, "npm test", sha));
        let cmds = ["npm test".to_string()];
        let failed = VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "tail".into(),
            ms: 1,
            ..Default::default()
        };
        task.fixture_data().base_sha = sha.to_string();
        let q = pre_existing_question(&[failed], sha);
        crate::engine::answer_policy::record_answered_question(
            &mut task,
            &q,
            "drop all",
            AnsweredBy::Policy,
        );
        record_owner_base_check(&mut task, &cmds, sha, "drop all", true);
        assert!(!base_check_answered(&task, "npm test", sha));
        crate::engine::answer_policy::record_answered_question(
            &mut task,
            &q,
            "drop all",
            AnsweredBy::Owner,
        );
        record_owner_base_check(&mut task, &cmds, sha, "drop all", false);
        assert_eq!(
            task.decisions.last().unwrap(),
            "Owner: base check npm test on 0123456 -> drop all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
        assert!(base_check_answered(&task, "npm test", "0123456ffff"));
        assert!(!base_check_answered(&task, "npm test", "fedcba9876"));
        assert!(!base_check_answered(&task, "npm run lint", sha));
    }

    #[test]
    fn an_orchestrator_answer_is_labelled_so_and_not_asked_again() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Queued);
        let sha = "0123456789abcdef";
        let cmds = ["npm test".to_string()];
        let failed = VerifyOutcome {
            command: "npm test".into(),
            code: Some(1),
            tail: "tail".into(),
            ms: 1,
            ..Default::default()
        };
        let q = pre_existing_question(&[failed], sha);
        crate::engine::answer_policy::record_answered_question(
            &mut task,
            &q,
            "keep all",
            AnsweredBy::Orchestrator,
        );
        record_owner_base_check(&mut task, &cmds, sha, "keep all", false);
        assert!(task.decisions.iter().all(|d| !d.starts_with("Owner:")));
        assert_eq!(
            task.decisions.last().unwrap(),
            "Orchestrator: base check npm test on 0123456 -> keep all"
        );
        assert!(base_check_answered(&task, "npm test", sha));
    }

    #[test]
    fn every_answerer_records_a_retry_on_the_exact_base() {
        let sha = "0123456789abcdef";
        let commands = ["npm test".to_string()];
        for who in [
            AnsweredBy::Owner,
            AnsweredBy::Orchestrator,
            AnsweredBy::Policy,
        ] {
            let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
            task.fixture_data().base_sha = sha.to_string();
            let question = pre_existing_question(
                &[VerifyOutcome {
                    command: commands[0].clone(),
                    code: Some(1),
                    tail: "failed".to_string(),
                    ms: 1,
                    ..Default::default()
                }],
                sha,
            );
            record_answered_question(&mut task, &question, PRE_EXISTING_RETRY, who);
            assert!(base_retry_requested(&task, &commands, sha));
            assert!(!base_retry_requested(&task, &commands, "0123456ffff"));
            assert!(!base_retry_requested(
                &task,
                &["npm run lint".to_string()],
                sha
            ));
        }
    }

    #[test]
    fn check_targets_finds_files_and_globs_only() {
        let t = |c: &str| check_targets(c);
        assert_eq!(
            t("node --test tests/a.test.cjs 'tests/b/*.cjs' \"src/c.ts\""),
            ["tests/a.test.cjs", "tests/b/*.cjs", "src/c.ts"]
        );
        assert_eq!(
            t("test -f ./Cargo.toml && grep -q x docs/[a-z]?.md"),
            ["Cargo.toml", "docs/[a-z]?.md"]
        );
        assert_eq!(t("cat build.log | wc -l > out.txt"), ["build.log"]);
        // Left out: flags, URLs, assignments, operators, bare words, versions.
        assert!(t("npm run test -- --reporter=dot -x").is_empty());
        assert!(t("cargo test -p sushiai-orch --test graph").is_empty());
        assert!(t("curl -fsS https://example.com/a/b.json").is_empty());
        assert!(t("CI=1 FILE=src/a.rs make check").is_empty());
        assert!(t("true && false || echo ok ; ls | cat < /dev/null").is_empty());
        assert!(t("node --version 18.2 2>/dev/null").is_empty());
    }

    #[test]
    fn task_target_detection_uses_the_command_and_changed_paths_but_ignores_ci_gates() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        task.fixture_data().request = Some("Make node --test tests/focus.test.cjs green".into());
        assert!(is_task_target(
            &task,
            "node --test tests/focus.test.cjs",
            &[]
        ));

        task.fixture_data().request = None;
        assert!(is_task_target(
            &task,
            "node --test crates/sushiai-orch/tests/base_retry.rs",
            &["crates/sushiai-orch/tests/base_retry.rs".into()]
        ));

        task.fixture_data().criteria = vec!["Checks pass -- check: npm run ci".into()];
        assert!(!is_task_target(&task, "npm run ci", &[]));
        task.fixture_data().criteria =
            vec!["Run the focused test -- check: node --test tests/base_retry.test.cjs".into()];
        assert!(is_task_target(
            &task,
            "node --test tests/base_retry.test.cjs",
            &[]
        ));
    }

    #[test]
    fn a_crate_name_in_the_request_does_not_make_a_shared_check_the_target() {
        // Live: a docs task named the crate "sushiai-orch"; a flaky test
        // "sushiai-orch::report ..." failed in the shared check on the base and
        // the task, and the check was kept as the task's own target.
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        task.fixture_data().request =
            Some("Document sushiai-orch and its engine modules in docs/MAP.md".into());
        task.fixture_data().goal = "Add docs/MAP.md describing sushiai-orch".into();
        task.fixture_data().criteria = vec!["The map exists -- check: test -f docs/MAP.md".into()];
        assert!(!command_named_by_task(&task, "npm run ci"));
        assert!(!is_task_target(&task, "npm run ci", &[]));
        assert!(!is_task_target(&task, "cargo test --workspace", &[]));
        // The planner's exact `-- check:` command still is the target.
        assert!(is_task_target(&task, "test -f docs/MAP.md", &[]));
    }

    #[test]
    fn a_name_in_the_check_output_alone_does_not_make_a_target() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        task.fixture_data().criteria = vec!["Checks pass -- check: npm run ci".into()];
        task.fixture_data().goal =
            "Fix the failing a_base_retry_carries_the_planner_test_commit test".into();
        // The output names a path the task changed and a token of the goal:
        // neither decides any more.
        assert!(!is_task_target(&task, "npm run ci", &[]));
        assert!(!is_task_target(
            &task,
            "cargo test",
            &["crates/sushiai-orch/tests/base_retry.rs".into()]
        ));
    }

    #[test]
    fn a_task_target_keep_survives_later_base_checks_and_new_wording() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        keep_check(&mut task, "cargo test", KEPT_BY_TASK_TARGET);
        task.fixture_data().decisions = vec!["reworded".into()];
        assert!(is_kept(&task, "cargo test"));
        assert!(!is_kept(&task, "npm test"));
        keep_check(&mut task, "cargo test", KEPT_BY_TASK_TARGET);
        assert_eq!(task.loop_state.kept_checks, vec!["cargo test".to_string()]);
    }

    #[test]
    fn a_base_answer_holds_after_its_decision_line_changes() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        let cmds = vec!["npm test".to_string()];
        record_base_answer(&mut task, &cmds, "0123456789", "Lead", PRE_EXISTING_RETRY);
        task.fixture_data().decisions = vec!["reworded".into()];
        assert!(base_check_answered(&task, "npm test", "0123456789"));
        assert!(!base_check_answered(&task, "npm test", "fffffff"));
        assert!(non_owner_base_retry(&task, "npm test", Some("0123456789")));
        assert!(non_owner_base_retry(&task, "npm test", None));
        assert!(base_retry_requested(&task, &cmds, "0123456789"));
        assert!(!base_retry_requested(&task, &cmds, "fffffff0"));
        // An owner's retry is a request but not a non-owner retry.
        let mut owned = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        record_base_answer(&mut owned, &cmds, "0123456789", "Owner", PRE_EXISTING_RETRY);
        assert!(!non_owner_base_retry(&owned, "npm test", None));
        assert!(base_retry_requested(&owned, &cmds, "0123456789"));
    }

    #[test]
    fn a_waited_out_base_stops_a_retry_whatever_the_lines_say() {
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Waiting);
        let cmds = vec!["npm test".to_string()];
        record_base_answer(&mut task, &cmds, "0123456789", "Owner", PRE_EXISTING_RETRY);
        task.fixture_data()
            .loop_state
            .waited_out_bases
            .push("0123456789".into());
        task.fixture_data().decisions.clear();
        assert!(!base_retry_requested(&task, &cmds, "0123456789"));
    }
}

#[cfg(test)]
mod answer_caller_tests {
    use super::*;

    #[test]
    fn a_retry_of_a_longer_command_does_not_cover_its_prefix_command() {
        let mut task = task_with_status(TaskStatus::Waiting);
        let base_sha = task.base_sha.clone();
        record_base_answer(
            &mut task,
            &["check on target".to_string()],
            &base_sha,
            "Lead",
            PRE_EXISTING_RETRY,
        );
        assert!(non_owner_base_retry(&task, "check on target", None));
        assert!(!non_owner_base_retry(&task, "check", None));
        assert!(!non_owner_base_retry(&task, "check", Some(&task.base_sha)));
    }

    #[test]
    fn amending_a_criterion_rebuilds_its_check_from_its_check_part() {
        let ck = |criterion: usize, run: &str| Check {
            criterion,
            run: run.into(),
            baseline: Some(Baseline::Fail),
        };
        let mut task = crate::engine::test_support::task_with_status(TaskStatus::Stopped);
        task.fixture_data().criteria = vec![
            "zero -- check: cargo test a".into(),
            "one -- check: cargo test b".into(),
        ];
        task.fixture_data().checks = vec![ck(0, "cargo test a"), ck(1, "cargo test b")];
        let amend = |task: &mut crate::task::state::Task,
                     criteria: Vec<&str>,
                     checks: Option<Vec<Check>>| {
            Amendment {
                criteria: Some(criteria.into_iter().map(String::from).collect()),
                checks,
                ..Default::default()
            }
            .apply_authorized(
                task,
                Origin::Owner,
                &crate::task::checks::authority::CheckAuthority::default(),
                now_ms(),
            )
            .unwrap();
        };
        let mut t = task.clone();
        amend(
            &mut t,
            vec!["zero -- check: cargo test a", "one -- check: cargo test c"],
            None,
        );
        assert_eq!(
            t.checks,
            vec![
                ck(0, "cargo test a"),
                Check {
                    criterion: 1,
                    run: "cargo test c".into(),
                    baseline: None
                }
            ]
        );
        assert!(t
            .decisions
            .iter()
            .any(|d| d.contains("rebuilt the check of criterion 1")));

        let mut t = task.clone();
        amend(
            &mut t,
            vec![
                "zero -- check: cargo test a",
                "one -- check: it passes with `cargo test d` run",
            ],
            None,
        );
        assert_eq!(t.checks[1].run, "cargo test d");

        // Prose before the command names no runnable command: the check stays.
        let mut t = task.clone();
        amend(
            &mut t,
            vec![
                "zero -- check: cargo test a",
                "x -- check: run cargo test foo",
            ],
            None,
        );
        assert_eq!(t.checks, task.checks);
        assert!(!t.decisions.iter().any(|d| d.contains("rebuilt")));

        // A reworded criterion with the same command keeps its check's baseline.
        let mut t = task.clone();
        amend(
            &mut t,
            vec![
                "zero -- check: cargo test a",
                "one, reworded -- check: cargo test b",
            ],
            None,
        );
        assert_eq!(t.checks, task.checks);
        assert!(!t.decisions.iter().any(|d| d.contains("rebuilt")));

        let mut t = task.clone();
        amend(
            &mut t,
            vec!["zero -- check: cargo test a", "one -- check: cargo test c"],
            Some(vec![ck(1, "explicit")]),
        );
        assert_eq!(t.checks.len(), 1);
        assert_eq!(t.checks[0].run, "explicit");
    }
}
