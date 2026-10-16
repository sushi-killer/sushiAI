use super::{carry::*, paths::*, prepare::*, restore::*, snapshot::*};
use crate::engine::graph::landing::Landing;
use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::harness::cancel::CancelToken;
use crate::policy::settings::SandboxMode;
use crate::task::{Check, FailureKind, PlannedTest, Task, TaskStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
mod cases {
    use super::*;

    fn test(path: &str, content: &str) -> PlannedTest {
        PlannedTest {
            path: path.into(),
            content: content.into(),
        }
    }

    #[test]
    fn only_repo_relative_paths_inside_the_limits_are_kept() {
        let big = "x".repeat(MAX_TEST_BYTES + 1);
        let mut input = vec![
            test("/etc/passwd", "a"),
            test("../up.rs", "a"),
            test("a/../b.rs", "a"),
            test(".git/hooks/pre-commit", "a"),
            test("sub/.git/config", "a"),
            test("  ", "a"),
            test("big.rs", &big),
            test("./tests/a.rs", "a"),
            test("tests/a.rs", "again"),
        ];
        let (kept, dropped) = keep_planned_tests(&input);
        assert_eq!(kept, vec![test("tests/a.rs", "a")]);
        assert_eq!(dropped.len(), 8, "{dropped:?}");
        assert!(dropped.iter().any(|d| d.contains("larger than 64 KiB")));
        input = (0..10).map(|i| test(&format!("t{i}.rs"), "")).collect();
        let (kept, dropped) = keep_planned_tests(&input);
        assert_eq!(kept.len(), 8);
        assert_eq!(dropped.len(), 2);
    }

    #[test]
    fn command_words_skip_flags_and_split_on_shell_punctuation() {
        assert_eq!(
            command_words("node --test tests/a.cjs && grep -q 'x' ./src/b.ts; cat <in/c.txt"),
            vec![
                "node",
                "tests/a.cjs",
                "grep",
                "x",
                "src/b.ts",
                "cat",
                "in/c.txt"
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_landing_check_keeps_the_planner_files_and_hashes_for_retry() {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/landing", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().planner_tests.files = vec![test("tests/planner.sh", "true\n")];
        let id = task.id.clone();
        let cancel = CancelToken::new();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &cancel).await);
        let old_hashes = task.planner_tests.protected.clone();
        std::fs::write(wt.join("implementation.txt"), "implementation\n").unwrap();
        std::fs::write(repo.join("FIXED.txt"), "fixed\n").unwrap();
        git_in(&repo, &["add", "FIXED.txt"]);
        git_in(&repo, &["commit", "-q", "-m", "fix: base"]);
        let new_base = crate::git::history::head_sha(&repo).unwrap();
        task.fixture_data().verify = vec!["false".to_string()];
        let run_dir = dir.path().join("runs");
        let result = crate::engine::graph::carry::CarryTree::new(&wt, &run_dir, None, &cancel)
            .onto(&app, &mut task, &base_ref, &[])
            .await
            .map_err(crate::engine::graph::carry::CarryFailure::into_landing);
        assert!(matches!(
            result,
            Err(Landing::Failed {
                kind: FailureKind::Verify,
                ..
            })
        ));
        assert_eq!(task.base_sha, new_base);
        assert_eq!(
            crate::git::history::head_sha(&wt).unwrap(),
            new_base,
            "no commit of ours"
        );
        assert_eq!(task.planner_tests.protected, old_hashes);
        assert_eq!(
            std::fs::read_to_string(wt.join("tests/planner.sh")).unwrap(),
            "true\n"
        );
        let changed = task.own_diff().changed_files(&wt).unwrap();
        assert_eq!(changed, vec!["implementation.txt"]);
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert!(wt.join("FIXED.txt").exists());

        task.fixture_data().verify.clear();
        assert!(matches!(
            crate::engine::graph::carry::CarryTree::new(&wt, &run_dir, None, &cancel)
                .onto(&app, &mut task, &base_ref, &[])
                .await
                .map_err(crate::engine::graph::carry::CarryFailure::into_landing),
            Ok(false)
        ));
        assert_eq!(crate::git::history::head_sha(&wt).unwrap(), new_base);
        let pending = crate::git::changes::changed_files(&wt, &new_base).unwrap();
        assert!(pending.contains(&"tests/planner.sh".to_string()));
        assert!(pending.contains(&"implementation.txt".to_string()));
    }

    fn repo_with(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        for (path, text) in files {
            let target = repo.join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text).unwrap();
        }
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "files"]);
        (dir, repo)
    }

    #[test]
    fn a_document_a_check_names_stays_writable_and_a_test_file_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("docs/guide.md", "old\n"),
            ("checks/t.test", "x\n"),
            ("notes/readme.rst", "r\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().criteria =
            vec!["Guide documents it -- check: grep -q parser docs/guide.md".into()];
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "grep -q parser docs/guide.md notes/readme.rst && run-tests checks/t.test".into(),
            baseline: None,
        }];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["checks/t.test"]);
    }

    #[test]
    fn a_document_a_check_compares_against_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("tests/golden/expected.md", "expected\n"),
            ("docs/guide.md", "old\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![
            Check {
                criterion: 0,
                run: "render-docs | diff - tests/golden/expected.md".into(),
                baseline: None,
            },
            Check {
                criterion: 0,
                run: "grep -q parser docs/guide.md && cmp tests/golden/expected.md out.md".into(),
                baseline: None,
            },
        ];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["tests/golden/expected.md"]);
    }

    #[test]
    fn a_document_read_as_a_pattern_file_or_through_a_substitution_stays_protected() {
        let (_dir, repo) = repo_with(&[
            ("tests/golden/expected.md", "expected\n"),
            ("docs/guide.md", "old\n"),
        ]);
        for run in [
            "render | grep -qxFf tests/golden/expected.md",
            "render | grep -f tests/golden/expected.md",
            "render | grep --file tests/golden/expected.md",
            "render | rg --file=tests/golden/expected.md",
            "[ \"$(render)\" = \"$(cat tests/golden/expected.md)\" ]",
            "grep -q \"$(cat tests/golden/expected.md)\" out.txt",
            "grep -q `cat tests/golden/expected.md` out.txt",
        ] {
            let mut task = task_with_status(TaskStatus::Running);
            task.fixture_data().checks = vec![Check {
                criterion: 0,
                run: run.into(),
                baseline: None,
            }];
            let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
            assert_eq!(kept, vec!["tests/golden/expected.md"], "{run}");
        }
        // The searched file of a plain probe stays writable.
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "grep -qE 'x' docs/guide.md".into(),
            baseline: None,
        }];
        assert!(protected_hashes(&repo, "HEAD", &task).is_empty());
    }

    #[tokio::test]
    async fn the_guard_restores_an_edited_test_and_reports_which_files() {
        let (_dir, repo) = repo_with(&[("t/a.test", "one\n"), ("t/b.test", "two\n")]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().planner_tests.files =
            vec![test("t/a.test", "one\n"), test("t/b.test", "two\n")];
        task.fixture_data().planner_tests.protected = Some(protected_hashes(&repo, "HEAD", &task));
        std::fs::write(repo.join("t/a.test"), "edited\n").unwrap();
        std::fs::remove_file(repo.join("t/b.test")).unwrap();
        let restored = guard_protected(&task, &repo, &[]).await.unwrap();
        assert_eq!(restored, vec!["t/a.test", "t/b.test"]);
        assert_eq!(
            std::fs::read_to_string(repo.join("t/a.test")).unwrap(),
            "one\n"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("t/b.test")).unwrap(),
            "two\n"
        );
        assert!(guard_protected(&task, &repo, &[]).await.is_none());
    }

    #[tokio::test]
    async fn a_carry_refreshes_the_protected_hashes_of_check_files_from_the_new_head() {
        let (_app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        std::fs::write(repo.join("check.sh"), "old\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-q", "-m", "check"]);
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/carry", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().criteria = vec!["it works".into()];
        task.fixture_data().checks = vec![Check {
            criterion: 0,
            run: "sh check.sh".into(),
            baseline: None,
        }];
        task.fixture_data().planner_tests.protected =
            Some(protected_hashes(&wt, &task.base_sha, &task));
        // The base moves and changes the protected file; the task did not.
        std::fs::write(repo.join("check.sh"), "new\n").unwrap();
        git_in(&repo, &["commit", "-qam", "newer check"]);
        std::fs::write(wt.join("impl.txt"), "work\n").unwrap();
        let mut snapshot = task.clone();
        let carried = carry_with_planner_tests(&mut snapshot, &wt, &base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        adopt_carried_tests(&mut task, snapshot.planner_tests.clone(), &carried);
        assert_eq!(
            task.planner_tests.protected.as_ref().unwrap()["check.sh"],
            sha256_hex(b"new\n")
        );
        assert!(
            task.decisions
                .iter()
                .any(|d| d.contains("protected check.sh now matches the new base")),
            "{:?}",
            task.decisions
        );
        if let crate::delivery::git::carry::Rebase::Moved { new_sha, .. } = carried {
            task.fixture_data().base_sha = new_sha;
        }
        let changed = task.own_diff().changed_files(&wt).unwrap();
        assert!(guard_protected(&task, &wt, &changed).await.is_none());
        assert_eq!(
            std::fs::read_to_string(wt.join("check.sh")).unwrap(),
            "new\n"
        );
    }

    fn check(run: &str) -> Check {
        Check {
            criterion: 0,
            run: run.into(),
            baseline: None,
        }
    }

    #[test]
    fn a_negated_or_wrapped_probe_still_counts_as_a_probe() {
        let (_dir, repo) = repo_with(&[("docs/a.md", "a\n"), ("tests/golden/expected.md", "g\n")]);
        for run in [
            "! grep -q X docs/a.md",
            "env A=b grep -q X docs/a.md",
            "FOO=1 /usr/bin/grep -q X docs/a.md",
            "env -i command grep -q X docs/a.md",
            "! test -s docs/a.md",
        ] {
            let mut task = task_with_status(TaskStatus::Running);
            task.fixture_data().checks = vec![check(run)];
            assert!(protected_hashes(&repo, "HEAD", &task).is_empty(), "{run}");
        }
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![check("! diff - tests/golden/expected.md")];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["tests/golden/expected.md"]);
    }

    #[test]
    fn a_file_a_check_wants_changed_is_writable_and_a_grepped_fixture_is_not() {
        let (_dir, repo) = repo_with(&[
            ("src/git/history.rs", "Command::new(\"git\")\n"),
            ("src/git/branches.rs", "Command::new(\"git\")\n"),
            ("src/git/command.rs", "fn run() {}\n"),
            ("src/prompts/clip.rs", "fn clip() {}\n"),
            ("data.txt", "fixture\n"),
            ("tests/y.test.cjs", "it(\"rejects empty\")\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().checks = vec![
            // An absence the base fails: the task must change these files.
            check("! rg -n 'Command::new(\"git\")' src/git/history.rs src/git/branches.rs"),
            check("test ! -e src/prompts/clip.rs"),
            // The brief's form: the path is grep's pattern, not a file it reads.
            check("{ git diff --name-only $ORCHD_BASE_SHA; git ls-files --others --exclude-standard; } | grep -qx 'src/git/command.rs'"),
            // A match the base already has guards a fixture.
            check("grep -q fixture data.txt && test -f impl.txt"),
            // A test file stays protected even under an absence probe.
            check("! rg -q 'it(\"rejects empty\")' tests/y.test.cjs"),
        ];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(kept, vec!["data.txt", "tests/y.test.cjs"]);
    }

    #[test]
    fn a_file_the_task_is_asked_to_change_is_not_protected_but_tests_and_fixtures_are() {
        let (_dir, repo) = repo_with(&[
            ("prompts/orchestrator.yaml", "x\n"),
            ("prompts/other.yaml", "y\n"),
            ("scripts/c.sh", "true\n"),
            ("tests/t.sh", "true\n"),
            ("tests/x.test.cjs", "z\n"),
        ]);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().title = "Change the prompt".into();
        task.fixture_data().goal =
            "Replace a line in prompts/orchestrator.yaml and add a tool.".into();
        task.fixture_data().criteria = vec![
            "The prompt is updated -- check: sh scripts/c.sh prompts/other.yaml".into(),
            "tests/x.test.cjs passes".into(),
        ];
        task.fixture_data().planner_tests.files = vec![test("tests/t.sh", "true\n")];
        task.fixture_data().checks = vec![
            check("sh tests/t.sh prompts/orchestrator.yaml"),
            check("sh scripts/c.sh prompts/other.yaml"),
            check("node --test tests/x.test.cjs"),
        ];
        let kept: Vec<String> = protected_hashes(&repo, "HEAD", &task).into_keys().collect();
        assert_eq!(
            kept,
            vec![
                "prompts/other.yaml",
                "scripts/c.sh",
                "tests/t.sh",
                "tests/x.test.cjs"
            ]
        );
    }

    #[tokio::test]
    async fn an_older_tasks_stale_protected_entries_are_dropped_before_an_attempt() {
        let (_app, dir) = test_app();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().goal = "Replace a line in prompts/orchestrator.yaml".into();
        task.fixture_data().checks = vec![check("sh tests/t.sh prompts/orchestrator.yaml")];
        task.fixture_data().planner_tests.protected = Some(
            [
                ("prompts/orchestrator.yaml".to_string(), "h1".to_string()),
                ("tests/t.sh".to_string(), "h2".to_string()),
            ]
            .into(),
        );
        repair_protection(&mut task, &repo).await;
        let kept: Vec<String> = task
            .planner_tests
            .protected
            .clone()
            .unwrap()
            .into_keys()
            .collect();
        assert_eq!(kept, vec!["tests/t.sh"]);
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("no longer protecting prompts/orchestrator.yaml")));
    }

    async fn prepared_with_format(
        setup_json: Option<&str>,
        content: &str,
    ) -> (Task, std::path::PathBuf, tempfile::TempDir) {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        if let Some(json) = setup_json {
            std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
            std::fs::write(repo.join(".sushiai/setup.json"), json).unwrap();
            git_in(&repo, &["add", "."]);
            git_in(&repo, &["commit", "-q", "-m", "setup"]);
        }
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/format", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.fixture_data().planner_tests.files = vec![test("tests/planner.txt", content)];
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        (task, wt, dir)
    }

    #[tokio::test]
    async fn planner_tests_are_formatted_by_the_repo_command_once_at_prepare() {
        let json = r#"{"format": "for f; do sed 's/ *$//' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done"}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \nb \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a\nb\n");
        assert!(task.planner_tests.commit.is_none());
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a\nb\n"
        );
        let hashes = task.planner_tests.protected.clone().unwrap();
        assert_eq!(hashes["tests/planner.txt"], sha256_hex(b"a\nb\n"));
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("formatted 1 planner test file(s)")));
    }

    #[tokio::test]
    async fn a_failing_format_command_stores_the_planner_tests_as_written() {
        let json = r#"{"format": ["sh", "-c", "echo mangled > \"$1\"; echo no formatter >&2; exit 2", "sh"]}"#;
        let (task, wt, _dir) = prepared_with_format(Some(json), "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a  \n"
        );
        assert!(task
            .decisions
            .iter()
            .any(|d| d.contains("format command failed on planner tests")
                && d.contains("stored as written")));
    }

    #[tokio::test]
    async fn without_a_format_command_the_planner_tests_stay_as_written() {
        let (task, wt, _dir) = prepared_with_format(None, "a  \n").await;
        assert_eq!(task.planner_tests.files[0].content, "a  \n");
        assert_eq!(
            std::fs::read(wt.join("tests/planner.txt")).unwrap(),
            b"a  \n"
        );
        assert!(!task.decisions.iter().any(|d| d.contains("format")));
    }

    struct Work {
        app: Arc<App>,
        task: Task,
        wt: std::path::PathBuf,
        repo: std::path::PathBuf,
        base_ref: String,
        _dir: tempfile::TempDir,
    }

    async fn prepared_work(files: Vec<PlannedTest>) -> Work {
        let (app, dir) = test_app();
        app.settings.write().unwrap().sandbox = SandboxMode::Host;
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        let base_ref = crate::git::branches::branch_of(&repo, "HEAD").unwrap();
        let wt = dir.path().join("task");
        let created =
            crate::git::worktree::create_worktree(&repo, "task/work", &wt, "HEAD").unwrap();
        let mut task = task_with_status(TaskStatus::Running);
        task.fixture_data().repo = repo.to_string_lossy().to_string();
        task.fixture_data().worktree = wt.to_string_lossy().to_string();
        task.fixture_data().base_sha = created.base_sha;
        task.record_base_ref(Some(base_ref.clone()));
        task.fixture_data().planner_tests.files = files;
        let id = task.id.clone();
        assert!(prepare_tests(&app, &id, &mut task, &wt, &CancelToken::new()).await);
        Work {
            app,
            task,
            wt,
            repo,
            base_ref,
            _dir: dir,
        }
    }

    fn commit_in(dir: &Path, file: &str, text: &str) -> String {
        std::fs::write(dir.join(file), text).unwrap();
        git_in(dir, &["add", "."]);
        git_in(dir, &["commit", "-q", "-m", "change"]);
        crate::git::history::head_sha(dir).unwrap()
    }

    #[tokio::test]
    async fn the_planner_tests_are_files_in_the_tree_never_a_commit() {
        let w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        assert!(w.task.planner_tests.commit.is_none());
        assert_eq!(
            crate::git::history::head_sha(&w.wt).unwrap(),
            w.task.base_sha
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            w.task.planner_tests.protected.clone().unwrap()["tests/planner.txt"],
            sha256_hex(b"expect\n")
        );
    }

    #[tokio::test]
    async fn an_implementer_commit_that_sweeps_the_planner_files_is_not_its_own_work() {
        let w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        git_in(&w.wt, &["add", "-A"]);
        git_in(&w.wt, &["commit", "-q", "-m", "agent commit"]);
        let own = w.task.own_diff().changed_files(&w.wt).unwrap();
        assert_eq!(
            own,
            vec!["impl.txt"],
            "planner files are in the commit but not in the diff"
        );
        let stat = w.task.own_diff().stat(&w.wt).unwrap();
        assert_eq!(stat.files, 1);
        let diff = w.task.own_diff().diff(&w.wt, 10_000).unwrap();
        assert!(
            diff.contains("impl.txt") && !diff.contains("planner.txt"),
            "{diff}"
        );
        // The guard writes the files over whatever the agent did to them.
        std::fs::write(w.wt.join("tests/planner.txt"), "weakened\n").unwrap();
        let restored = guard_protected(&w.task, &w.wt, &own).await.unwrap();
        assert_eq!(restored, vec!["tests/planner.txt"]);
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
    }

    #[tokio::test]
    async fn a_carry_onto_a_moved_base_works_with_the_planner_tests_present() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        let new_base = commit_in(&w.repo, "OTHER.txt", "other\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(&carried, crate::delivery::git::carry::Rebase::Moved { new_sha, .. } if *new_sha == new_base),
            "{carried:?}"
        );
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), new_base);
        let read = |f: &str| std::fs::read_to_string(w.wt.join(f)).unwrap();
        assert_eq!(read("tests/planner.txt"), "expect\n");
        assert_eq!(read("impl.txt"), "work\n");
        assert_eq!(read("OTHER.txt"), "other\n");
    }

    #[tokio::test]
    async fn a_planner_path_the_base_added_since_the_task_began_does_not_block_the_carry() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        std::fs::create_dir_all(w.repo.join("tests")).unwrap();
        let new_base = commit_in(&w.repo, "tests/planner.txt", "the base wrote this\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(&carried, crate::delivery::git::carry::Rebase::Moved { new_sha, .. } if *new_sha == new_base),
            "{carried:?}"
        );
        // The protected file is the planner's, not the base's.
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("impl.txt")).unwrap(),
            "work\n"
        );
        let changed = w.task.own_diff().changed_files(&w.wt).unwrap();
        assert!(guard_protected(&w.task, &w.wt, &changed).await.is_none());
    }

    #[tokio::test]
    async fn a_task_with_an_old_tests_commit_is_migrated_and_then_carries() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        // What an older build left: the planner files and the agent's work as commits.
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        w.task.fixture_data().planner_tests.commit =
            Some(crate::git::history::head_sha(&w.wt).unwrap());
        std::fs::write(w.wt.join("impl.txt"), "work\n").unwrap();
        let new_base = commit_in(&w.repo, "OTHER.txt", "other\n");

        repair_protection(&mut w.task, &w.wt).await;
        assert!(w.task.planner_tests.commit.is_none());
        assert_eq!(
            crate::git::history::head_sha(&w.wt).unwrap(),
            w.task.base_sha
        );
        assert!(w
            .task
            .decisions
            .iter()
            .any(|d| d.contains("no longer a commit of the branch")));

        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), new_base);
        assert_eq!(
            std::fs::read_to_string(w.wt.join("tests/planner.txt")).unwrap(),
            "expect\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.wt.join("impl.txt")).unwrap(),
            "work\n"
        );
    }

    #[tokio::test]
    async fn a_carry_with_an_old_tests_commit_migrates_first() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        w.task.fixture_data().planner_tests.commit =
            Some(crate::git::history::head_sha(&w.wt).unwrap());
        commit_in(&w.repo, "OTHER.txt", "other\n");
        let carried = carry_with_planner_tests(&mut w.task, &w.wt, &w.base_ref).unwrap();
        assert!(
            matches!(carried, crate::delivery::git::carry::Rebase::Moved { .. }),
            "{carried:?}"
        );
        assert!(w.task.planner_tests.commit.is_none());
    }

    #[tokio::test]
    async fn a_finished_task_keeps_its_branch_when_loaded() {
        let mut w = prepared_work(vec![test("tests/planner.txt", "expect\n")]).await;
        git_in(&w.wt, &["add", "tests/planner.txt"]);
        git_in(&w.wt, &["commit", "-q", "-m", "test: planner tests"]);
        let tests = crate::git::history::head_sha(&w.wt).unwrap();
        w.task.fixture_data().planner_tests.commit = Some(tests.clone());
        w.task.fixture_data().pr_url = Some("https://example.invalid/pr/1".into());
        repair_protection(&mut w.task, &w.wt).await;
        assert_eq!(w.task.planner_tests.commit.as_deref(), Some(tests.as_str()));
        assert_eq!(crate::git::history::head_sha(&w.wt).unwrap(), tests);
        let _ = &w.app;
    }

    #[test]
    fn the_base_with_the_planner_tests_is_a_commit_no_branch_holds() {
        let (_dir, repo) = repo_with(&[("a.txt", "a\n")]);
        let base = crate::git::history::head_sha(&repo).unwrap();
        let files = vec![("t/new.txt".to_string(), b"new\n".to_vec())];
        let over = crate::delivery::git::commit::commit_over(&repo, &base, &files).unwrap();
        assert_ne!(over, base);
        assert_eq!(
            crate::git::history::blob_at(&repo, &over, "t/new.txt").unwrap(),
            b"new\n"
        );
        assert_eq!(
            crate::git::history::blob_at(&repo, &over, "a.txt").unwrap(),
            b"a\n"
        );
        assert_eq!(
            crate::delivery::git::commit::commit_over(&repo, &base, &files).unwrap(),
            over
        );
        assert_eq!(crate::git::history::head_sha(&repo).unwrap(), base);
    }
}
