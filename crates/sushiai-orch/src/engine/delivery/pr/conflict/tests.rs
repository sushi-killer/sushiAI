use super::*;
use crate::clock::now_ms;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{Assumption, QuestionKind, Task, TaskStatus};
use std::path::Path;

#[test]
fn an_owner_change_before_the_conflict_queue_is_not_replaced_by_the_old_done_task() {
    reproduce("initial", "engine::delivery::pr::conflict::tests::an_owner_change_before_the_conflict_queue_is_not_replaced_by_the_old_done_task");
}

#[test]
fn a_current_conflict_after_setup_keeps_its_owner_and_actor_while_a_stop_stays_stopped() {
    let exact = "engine::delivery::pr::conflict::tests::a_current_conflict_after_setup_keeps_its_owner_and_actor_while_a_stop_stays_stopped";
    if std::env::var_os("ORCHD_TEST_CONFLICT_ROOT").is_some() {
        reproduce("setup", exact);
        return;
    }
    for mode in ["setup", "setup-stop"] {
        reproduce(mode, exact);
    }
}

fn reproduce(mode: &str, exact: &str) {
    if let Some(root) = std::env::var_os("ORCHD_TEST_CONFLICT_ROOT") {
        let mode = std::env::var("ORCHD_TEST_CONFLICT_MODE").unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_conflict(Path::new(&root), &mode));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let git = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(git.status.success());
    write_git(dir.path(), String::from_utf8(git.stdout).unwrap().trim());
    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([exact, "--exact", "--nocapture"])
        .env("ORCHD_TEST_CONFLICT_ROOT", dir.path())
        .env("ORCHD_TEST_CONFLICT_MODE", mode)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated conflict caller failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_git(root: &Path, git: &str) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        r#"#!/bin/sh
root={root}
if [ "$ORCHD_TEST_CONFLICT_MODE" = initial ] && [ "$1" = rev-parse ] && [ "$2" = origin/main ] && [ ! -e "$root/released" ]; then
 : > "$root/at-boundary"
 while [ ! -e "$root/released" ]; do sleep 0.01; done
fi
exec {git} "$@"
"#,
        root = shell_path(root),
        git = shell_path(Path::new(git))
    );
    let path = root.join("git");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

async fn run_conflict(root: &Path, mode: &str) {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let (task, tip) = fixture(dir.path(), root, mode != "initial");
    app.store.save_task(&task).unwrap();
    let (worker_app, id, initial) = (app.clone(), task.id.clone(), mode == "initial");
    let observed = task.clone();
    let worker = tokio::spawn(async move {
        if initial {
            crate::engine::delivery::pr::carry::carry_for_pr(&worker_app, &observed, None)
                .await
                .unwrap()
        } else {
            resolve_conflict(&worker_app, &id, "main", None)
                .await
                .unwrap()
        }
    });
    crate::engine::test_support::wait_async(
        "the actual conflict caller to reach its barrier",
        || root.join("at-boundary").exists(),
    )
    .await;
    if mode == "setup-stop" {
        app.dispatch("task.stop", serde_json::json!({"id":task.id}))
            .await
            .unwrap();
    } else {
        app.dispatch(
            "task.overturn",
            serde_json::json!({
                "id":task.id,"index":0,"answer":"Keep the failed check"
            }),
        )
        .await
        .unwrap();
    }
    std::fs::write(root.join("released"), "").unwrap();
    let result = worker.await.unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    if mode == "setup-stop" {
        assert!(matches!(result, PrCarry::Deferred));
        assert_eq!(saved.status, TaskStatus::Stopped);
        assert!(!app.controls.lock().unwrap().contains_key(&task.id));
        return;
    }
    assert!(
        saved.assumptions[0].overturned,
        "conflict admission restored the revoked Owner answer"
    );
    assert!(
        matches!(result, PrCarry::Resolving(_) | PrCarry::ChecksPending),
        "the queued conflict was silently deferred"
    );
    assert_eq!(saved.status, TaskStatus::Queued);
    assert!(
        app.controls.lock().unwrap().contains_key(&task.id),
        "the current conflict has no actor"
    );
    assert!(saved.pr_pending && saved.pr_url.is_none() && saved.queue.pr_conflict_tried);
    assert_eq!(
        saved
            .fresh_base
            .as_ref()
            .and_then(|fresh| fresh.tip.as_ref()),
        Some(&tip)
    );
    let unresolved =
        std::fs::read_to_string(Path::new(&saved.worktree).join("shared.txt")).unwrap();
    assert!(
        unresolved.contains("<<<<<<<")
            && unresolved.contains("task intent")
            && unresolved.contains("new base intent"),
        "{unresolved}"
    );
    app.dispatch("task.stop", serde_json::json!({"id":task.id}))
        .await
        .unwrap();
}

fn fixture(repo_root: &Path, tools: &Path, setup_barrier: bool) -> (Task, String) {
    let repo = repo_root.join("repo");
    std::fs::create_dir_all(repo.join(".sushiai")).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    let setup = if setup_barrier {
        format!(
        "if [ \"${{PWD##*/}}\" = conflict-worktree ]; then : > {}; while [ ! -e {} ]; do sleep 0.01; done; fi",
        shell_path(&tools.join("at-boundary")), shell_path(&tools.join("released"))
    )
    } else {
        "true".into()
    };
    std::fs::write(
        repo.join(".sushiai/setup.json"),
        serde_json::json!({"setup":setup}).to_string(),
    )
    .unwrap();
    std::fs::write(repo.join("shared.txt"), "original\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "conflict fixture"]);
    let base = git_in(&repo, &["rev-parse", "HEAD"]);
    git_in(&repo, &["checkout", "-qb", "task/conflict"]);
    std::fs::write(repo.join("shared.txt"), "task intent\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "task intent"]);
    git_in(&repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("shared.txt"), "new base intent\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "base intent"]);
    let tip = git_in(&repo, &["rev-parse", "HEAD"]);
    git_in(&repo, &["remote", "add", "origin", repo.to_str().unwrap()]);
    git_in(&repo, &["fetch", "-q", "origin", "main"]);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().worktree = repo_root
        .join("conflict-worktree")
        .to_string_lossy()
        .into_owned();
    task.fixture_data().branch = "task/conflict".into();
    task.fixture_data().base_sha = base;
    task.fixture_data().base_ref = Some("main".into());
    task.fixture_data().queue.paused_until = Some(now_ms() + 60_000);
    task.record_pr_pending(true);
    task.record_assumption(Assumption {
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
    (task, tip)
}
