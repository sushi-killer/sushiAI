use crate::engine::answer_policy::is_shared_check;
use crate::engine::test_support::git_in;
use crate::engine::test_support::init_git_repo;
use crate::engine::test_support::task_with_status;
use crate::engine::test_support::test_app;
use crate::engine::verification::scoping::scope_task_checks;
use crate::engine::verification::scoping::scoped_checks_for;
use crate::git::paths::short_sha;
use crate::policy::settings::GateAt;
use crate::policy::settings::ScopedCheck;
use crate::policy::settings::Settings;
use crate::task::state::TaskStatus;
use crate::verify::scoping::filter_scoped;
use crate::verify::scoping::filter_scoped_planned;

#[cfg(test)]
mod scoped_task_tests {
    use super::*;

    fn repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".sushiai")).unwrap();
        std::fs::write(
            dir.path().join(".sushiai/setup.json"),
            r#"{"scopedChecks":[{"command":"repo-check","paths":["src/**"]}]}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("a/src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = ['a']\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a/Cargo.toml"),
            "[package]\nname = 'package-a'\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// base\n").unwrap();
        init_git_repo(dir.path());
        let base = git_in(dir.path(), &["rev-parse", "HEAD"]);
        (dir, base)
    }

    #[test]
    fn merged_base_policy_is_shared_and_uses_the_existing_filters() {
        let (dir, base) = repo();
        let task = crate::task::test_support::fixture(crate::task::state::TaskData {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base,
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let mut settings = Settings::default();
        settings.scoped_checks.push(ScopedCheck {
            repo: None,
            command: "repo-check".into(),
            paths: vec!["electron/**".into()],
        });
        std::fs::write(
            dir.path().join(".sushiai/setup.json"),
            r#"{"scopedChecks":[{"command":"new-check","paths":["never/**"]}]}"#,
        )
        .unwrap();
        let scoped = scoped_checks_for(&settings, &task);
        assert_eq!(scoped.len(), 2);
        assert_eq!(scoped[1].repo.as_deref(), Some(task.repo.as_str()));
        assert_eq!(scoped[1].paths, ["src/**"]);
        assert!(is_shared_check(&Settings::default(), &task, "repo-check"));
        let commands = vec!["repo-check".into()];
        for file in ["src/a.ts", "electron/main.cjs"] {
            assert_eq!(
                filter_scoped(&scoped, &task.repo, &commands, &[file.into()]).0,
                commands
            );
            assert_eq!(
                filter_scoped_planned(&scoped, &task.repo, &commands, &[file.into()]),
                commands
            );
        }
        assert!(
            filter_scoped(&scoped, &task.repo, &commands, &["other/file".into()])
                .0
                .is_empty()
        );
        assert!(
            filter_scoped_planned(&scoped, &task.repo, &commands, &["other".into()]).is_empty()
        );
    }

    #[tokio::test]
    async fn scoped_checks_are_persisted_to_the_parent_once_and_never_lost() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// changed\n").unwrap();
        let parent = task_with_status(TaskStatus::Running);
        app.store.save_task(&parent).unwrap();
        let mut child = crate::task::test_support::fixture(crate::task::state::TaskData {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base.clone(),
            parent: Some(parent.id.clone()),
            brief_check: crate::task::planning::BriefCheck {
                check: Some("cargo test -p package-a".into()),
                ..Default::default()
            },
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let commands = vec!["cargo test --workspace".into(), "repo-check".into()];
        let (run, lines) = scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        assert_eq!(run, ["cargo test -p package-a"]);
        assert!(lines.contains(&"Orchestrator: subtask ran cargo test -p package-a instead of cargo test --workspace; the parent runs cargo test --workspace on the combined tree".into()));
        let saved = app.store.load_task(&parent.id).unwrap().unwrap();
        assert_eq!(saved.final_verify, commands);
        assert_eq!(saved.decisions.len(), 2);
        assert!(saved.decisions.contains(&format!(
            "Orchestrator: added cargo test --workspace to finalVerify for subtask {}",
            short_sha(&child.id)
        )));
        scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        let saved_again = app.store.load_task(&parent.id).unwrap().unwrap();
        assert_eq!(saved_again.final_verify, saved.final_verify);
        assert_eq!(saved_again.decisions, saved.decisions);
        child.fixture_data().parent = Some("missing-parent".into());
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands).await,
            (commands.clone(), Vec::new())
        );
        child.detach_parent();
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands)
                .await
                .0,
            ["cargo test --workspace"]
        );
    }

    #[tokio::test]
    async fn a_subtask_runs_its_planner_check_in_place_of_the_ci_entry() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// changed\n").unwrap();
        let parent = task_with_status(TaskStatus::Running);
        app.store.save_task(&parent).unwrap();
        let child = crate::task::test_support::fixture(crate::task::state::TaskData {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base.clone(),
            parent: Some(parent.id.clone()),
            brief_check: crate::task::planning::BriefCheck {
                check: Some("go test ./pkg/x".into()),
                ..Default::default()
            },
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let commands = vec!["npm run ci".into()];
        let (run, lines) = scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        assert_eq!(run, ["go test ./pkg/x"]);
        assert!(lines.contains(&"Orchestrator: subtask ran go test ./pkg/x instead of npm run ci; the parent runs npm run ci on the combined tree".into()));
        let saved = app.store.load_task(&parent.id).unwrap().unwrap();
        assert_eq!(saved.final_verify, ["npm run ci"]);
    }

    #[tokio::test]
    async fn a_subtask_never_runs_a_test_matched_only_by_a_file_stem() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/index.test.cjs"), "").unwrap();
        std::fs::write(dir.path().join("a/src/index.js"), "// changed\n").unwrap();
        git_in(dir.path(), &["add", "-A"]);
        let parent = task_with_status(TaskStatus::Running);
        app.store.save_task(&parent).unwrap();
        let child = crate::task::test_support::fixture(crate::task::state::TaskData {
            repo: dir.path().to_string_lossy().into_owned(),
            base_sha: base.clone(),
            parent: Some(parent.id.clone()),
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let commands = vec!["npm run ci".into()];
        let (run, _) = scope_task_checks(&app, &child, dir.path(), &base, &commands).await;
        assert_eq!(run, ["npm run ci"]);
        let planned = crate::task::test_support::fixture(crate::task::state::TaskData {
            brief_check: crate::task::planning::BriefCheck {
                check: Some("node --test tests/parser.test.cjs".into()),
                ..Default::default()
            },
            ..(*(child)).clone()
        });
        let (run, _) = scope_task_checks(&app, &planned, dir.path(), &base, &commands).await;
        assert_eq!(run, ["node --test tests/parser.test.cjs"]);
    }

    #[tokio::test]
    async fn every_gate_disables_subtask_narrowing() {
        let (app, _app_dir) = test_app();
        let (dir, base) = repo();
        std::fs::write(dir.path().join("a/src/lib.rs"), "// changed\n").unwrap();
        let repo = dir.path().to_string_lossy().into_owned();
        app.settings
            .write()
            .unwrap()
            .gate_at
            .insert(repo.clone(), GateAt::Every);
        let child = crate::task::test_support::fixture(crate::task::state::TaskData {
            repo,
            base_sha: base.clone(),
            parent: Some("parent".into()),
            ..(*(task_with_status(TaskStatus::Running))).clone()
        });
        let commands = vec!["cargo test --workspace".into()];
        assert_eq!(
            scope_task_checks(&app, &child, dir.path(), &base, &commands).await,
            (commands, Vec::new())
        );
    }
}
