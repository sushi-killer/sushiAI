use crate::engine::state::App;
use crate::engine::test_support::{git_in, init_git_repo, task_with_status, test_app};
use crate::policy::settings::SandboxMode;
use crate::task::{Assumption, QuestionKind, Task, TaskStatus, VerifyOutcome};
use std::path::Path;
use std::sync::Arc;

#[test]
fn an_owner_revocation_before_the_final_prepare_cannot_use_the_old_carry_admission() {
    reproduce("contains", "engine::delivery::pr::open::tests::an_owner_revocation_before_the_final_prepare_cannot_use_the_old_carry_admission");
}

#[test]
fn an_owner_revocation_while_the_final_body_is_built_rechecks_before_pushing() {
    reproduce("body", "engine::delivery::pr::open::tests::an_owner_revocation_while_the_final_body_is_built_rechecks_before_pushing");
}

#[test]
fn an_owner_revocation_during_push_prereads_rechecks_before_the_git_write() {
    reproduce("push", "engine::delivery::pr::open::tests::an_owner_revocation_during_push_prereads_rechecks_before_the_git_write");
}

#[test]
fn an_owner_revocation_during_pr_lookup_rechecks_before_the_gh_write() {
    reproduce("gh-list", "engine::delivery::pr::open::tests::an_owner_revocation_during_pr_lookup_rechecks_before_the_gh_write");
}

fn reproduce(mode: &str, exact: &str) {
    if let Some(root) = std::env::var_os("ORCHD_TEST_PR_WINDOW_ROOT") {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(run_window(Path::new(&root)));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let git = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(git.status.success());
    write_tools(dir.path(), String::from_utf8(git.stdout).unwrap().trim());
    let mut paths = vec![dir.path().to_path_buf()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([exact, "--exact", "--nocapture"])
        .env("ORCHD_TEST_PR_WINDOW_ROOT", dir.path())
        .env("ORCHD_TEST_PR_WINDOW_MODE", mode)
        .env("ORCHD_GH_BIN", dir.path().join("gh"))
        .env("PATH", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated caller failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_tools(root: &Path, git: &str) {
    let script = format!(
        r#"#!/bin/sh
root={root}
if [ "$1" = merge-base ] && [ "$2" = --is-ancestor ]; then
 n=0; if [ -e "$root/contains-count" ]; then n=$(cat "$root/contains-count"); fi
 n=$((n+1)); printf '%s' "$n" > "$root/contains-count"
 if [ "$ORCHD_TEST_PR_WINDOW_MODE" = contains ] && [ "$n" = 2 ]; then wait_here=yes; fi
fi
if [ "$ORCHD_TEST_PR_WINDOW_MODE" = body ] && [ "$1" = log ] && [ "$2" = --all ]; then wait_here=yes; fi
if [ "$ORCHD_TEST_PR_WINDOW_MODE" = push ] && [ "$1" = ls-remote ] && [ "$2" = --heads ]; then wait_here=yes; fi
if [ "$wait_here" = yes ] && [ ! -e "$root/released" ]; then
 : > "$root/at-boundary"
 while [ ! -e "$root/released" ]; do sleep 0.01; done
fi
if [ "$1" = push ] && [ "$2" = -u ]; then : > "$root/push-attempted"; fi
exec {git} "$@"
"#,
        root = shell_path(root),
        git = shell_path(Path::new(git))
    );
    executable(&root.join("git"), &script);
    let gh = format!(
        r#"#!/bin/sh
root={root}
if [ "$ORCHD_TEST_PR_WINDOW_MODE" = gh-list ] && [ "$1 $2" = 'pr list' ] && [ ! -e "$root/released" ]; then
 : > "$root/at-boundary"
 while [ ! -e "$root/released" ]; do sleep 0.01; done
fi
if [ "$1 $2" = 'pr create' ]; then
 : > {created}
 printf 'https://github.example/acme/app/pull/7\n'
elif [ "$1 $2" = 'pr view' ]; then
 head=$({git} rev-parse refs/heads/task/window)
 printf '{{"url":"https://github.example/acme/app/pull/7","headRefOid":"%s","title":"feat: scoped change"}}\n' "$head"
fi
"#,
        root = shell_path(root),
        created = shell_path(&root.join("pr-created")),
        git = shell_path(Path::new(git))
    );
    executable(&root.join("gh"), &gh);
}

fn executable(path: &Path, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

async fn run_window(root: &Path) {
    let (app, dir) = test_app();
    app.settings.write().unwrap().sandbox = SandboxMode::Host;
    let task = fixture(&app, dir.path());
    let (worker_app, id) = (app.clone(), task.id.clone());
    let worker = tokio::spawn(async move {
        worker_app
            .open_pr(
                crate::delivery::pr::request::PrParams {
                    id,
                    title: Some("feat: scoped change".into()),
                    branch: None,
                    body: Some("Actual scoped work".into()),
                    source: None,
                },
                true,
            )
            .await
    });
    crate::engine::test_support::wait_async(
        "open_pr to reach its post-carry publication barrier",
        || root.join("at-boundary").exists(),
    )
    .await;
    app.dispatch(
        "task.overturn",
        serde_json::json!({
            "id":task.id,"index":0,"answer":"Keep the failed check and repair it"
        }),
    )
    .await
    .unwrap();
    std::fs::write(root.join("released"), "").unwrap();
    let result = worker.await.unwrap();
    let saved = app.store.load_task(&task.id).unwrap().unwrap();
    if std::env::var("ORCHD_TEST_PR_WINDOW_MODE").unwrap() == "gh-list" {
        assert!(
            root.join("push-attempted").exists(),
            "the barrier must follow the accepted push"
        );
    } else {
        assert!(
            !root.join("push-attempted").exists(),
            "a revoked failed check was pushed: {result:?}"
        );
    }
    assert!(!root.join("pr-created").exists() && saved.pr_url.is_none());
    assert!(result.is_err());
    assert_eq!(
        saved.status,
        TaskStatus::Waiting,
        "the current failure has no owner: {saved:?}"
    );
    assert!(saved.pr_pending && saved.assumptions[0].overturned);
    assert_eq!(saved.assumptions.len(), 1);
    let question = saved.question.as_ref().unwrap();
    assert_eq!(question.kind, QuestionKind::PrCheck);
    assert!(question.for_owner && question.commands == ["sh ci.sh"]);
    assert!(saved.queue.pr_check_repair.is_some());
}

fn fixture(app: &Arc<App>, root: &Path) -> Task {
    let repo = root.join("repo");
    let remote = root.join("origin.git");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);
    git_in(&repo, &["branch", "-M", "main"]);
    std::fs::write(
        repo.join("ci.sh"),
        "printf 'not ok 1 - owner_case_17\\n'\nexit 1\n",
    )
    .unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "check fixture"]);
    let base = git_in(&repo, &["rev-parse", "HEAD"]);
    git_in(root, &["init", "--bare", "-q", remote.to_str().unwrap()]);
    git_in(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git_in(
        &repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git_in(&repo, &["push", "-qu", "origin", "main"]);
    git_in(&repo, &["checkout", "-qb", "task/window"]);
    std::fs::write(repo.join("task.txt"), "actual task work\n").unwrap();
    git_in(&repo, &["add", "."]);
    git_in(&repo, &["commit", "-qm", "task work"]);
    let head = git_in(&repo, &["rev-parse", "HEAD"]);
    let tree = git_in(&repo, &["rev-parse", "HEAD^{tree}"]);
    git_in(&repo, &["checkout", "-q", "main"]);
    let mut task = task_with_status(TaskStatus::Done);
    task.fixture_data().repo = repo.to_string_lossy().into_owned();
    task.fixture_data().branch = "task/window".into();
    task.fixture_data().base_sha = base;
    task.fixture_data().final_verify = vec!["sh ci.sh".into()];
    task.record_pr_pending(true);
    task.record_delivery_checks(vec![VerifyOutcome {
        command: "sh ci.sh".into(),
        code: Some(1),
        tail: "not ok 1 - owner_case_17".into(),
        failing: vec!["owner_case_17".into()],
        commit: Some(head),
        tree: Some(tree),
        output_key: crate::policy::test_output::parse("not ok 1 - owner_case_17\n", "").rest_key,
        ..Default::default()
    }]);
    task.record_assumption(Assumption {
        question: "Accept the inherited check?".into(),
        answer: crate::task::questions::WAIVED.into(),
        evidence: "owner_case_17 fails on the base".into(),
        by: crate::task::questions::WAIVER_BY.into(),
        kind: Some(QuestionKind::PreexistingFailure),
        attempt: None,
        commands: vec!["sh ci.sh".into()],
        overturned: false,
        owner_answer: None,
    });
    app.store.save_task(&task).unwrap();
    task
}
