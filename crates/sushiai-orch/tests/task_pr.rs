//! Black-box integration tests for `task.pr`: the owner pushes a done task's
//! branch to `origin` and opens a pull request through a (fake) `gh`.

mod common;

use common::*;
use std::path::Path;
use std::time::Duration;

const SCRIPT: &str = r#"#!/bin/sh
echo pr.txt > pr.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

/// Records argv of every call. `pr list` is empty until `pr create` ran;
/// `pr create` fails with stderr while the file `fail` exists.
const FAKE_GH: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
printf '%s\n' "$*" >> "$dir/gh.log"
case "$1 $2" in
  "pr list") [ -f "$dir/created" ] && echo https://github.example/acme/app/pull/7; exit 0 ;;
  "pr create")
    if [ -f "$dir/fail" ]; then echo "boom from gh" >&2; exit 1; fi
    touch "$dir/created"; echo https://github.example/acme/app/pull/7 ;;
esac
"#;

const URL: &str = "https://github.example/acme/app/pull/7";

struct Setup {
    daemon: Daemon,
    tools: tempfile::TempDir,
    repo: tempfile::TempDir,
    remote: tempfile::TempDir,
    id: String,
}

impl Setup {
    fn gh_log(&self) -> String {
        std::fs::read_to_string(self.tools.path().join("gh.log")).unwrap_or_default()
    }
    fn remote_main(&self) -> String {
        git_out(self.remote.path(), &["rev-parse", "main"])
    }
    fn pr(&self, branch: &str) -> String {
        self.daemon.request_error(
            "task.pr",
            serde_json::json!({"id": self.id, "branch": branch}),
        )
    }
}

fn create(daemon: &Daemon, repo: &Path, start: bool) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": "Add pr file",
            "goal": "write pr.txt",
            "criteria": ["pr.txt exists"],
            "verify": ["test -f pr.txt"],
            "start": start,
        }),
    )
}

/// A daemon with fake harness and gh, a repo whose `main` is on a bare
/// `origin`, and a done task with one commit.
fn setup() -> Setup {
    let tools = tempfile::tempdir().unwrap();
    let harness = fake_harness_script(tools.path(), "fake-claude.sh", SCRIPT);
    let gh = fake_harness_script(tools.path(), "fake-gh.sh", FAKE_GH);
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", harness.to_str().unwrap()),
        ("ORCHD_GH_BIN", gh.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    cheap_route_on_claude(&daemon);

    let repo = init_git_repo();
    git_out(repo.path(), &["branch", "-M", "main"]);
    let remote = tempfile::tempdir().unwrap();
    git_out(remote.path(), &["init", "-q", "--bare", "-b", "main"]);
    git_out(
        repo.path(),
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    git_out(repo.path(), &["push", "-q", "origin", "main"]);

    let id = create(&daemon, repo.path(), true)["id"]
        .as_str()
        .unwrap()
        .to_string();
    let done = poll_until(&daemon, &id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    });
    assert_eq!(done["status"], "done", "{done}");
    Setup {
        daemon,
        tools,
        repo,
        remote,
        id,
    }
}

#[test]
fn task_pr_pushes_the_branch_opens_one_pull_request_and_refuses_a_task_that_is_not_done() {
    let s = setup();
    let idle = create(&s.daemon, s.repo.path(), false);
    let err = s
        .daemon
        .request_error("task.pr", serde_json::json!({"id": idle["id"]}));
    assert!(err.contains("only a done task"), "{err}");

    let first = s.daemon.request(
        "task.pr",
        serde_json::json!({"id": s.id, "title": "Add the PR file", "branch": "feature/pr-file"}),
    );
    assert_eq!(first["prUrl"], URL, "{first}");
    assert!(
        git_out(s.remote.path(), &["branch", "--list", "feature/pr-file"])
            .contains("feature/pr-file"),
        "branch missing on the remote"
    );
    let log = s.gh_log();
    assert!(
        log.contains("pr list --head feature/pr-file --state open"),
        "{log}"
    );
    assert!(
        log.contains("pr create --base main --head feature/pr-file --title Add the PR file"),
        "{log}"
    );
    assert!(log.contains("- [x] pr.txt exists"), "{log}");
    assert!(first["decisions"]
        .to_string()
        .contains(&format!("PR: opened {URL}")));
    let stored = s
        .daemon
        .request("task.get", serde_json::json!({"id": s.id}));
    assert_eq!(stored["prUrl"], URL);

    // Again: the open pull request is reused and updated, nothing is created.
    let second = s.daemon.request(
        "task.pr",
        serde_json::json!({"id": s.id, "branch": "feature/pr-file"}),
    );
    assert_eq!(second["prUrl"], URL);
    assert!(second["decisions"]
        .to_string()
        .contains(&format!("PR: updated {URL}")));
    assert_eq!(s.gh_log().matches("pr create").count(), 1, "{}", s.gh_log());
}

#[test]
fn task_pr_refuses_unsafe_branch_names_and_never_touches_the_base_branch() {
    let s = setup();
    let before = s.remote_main();
    for bad in ["main", "-x", "refs/heads/x", "a:b", "+x", "HEAD", "a b"] {
        let err = s.pr(bad);
        assert!(err != "null" && !err.is_empty(), "{bad} accepted");
    }
    assert!(s.pr("main").contains("is a base branch"));
    assert_eq!(s.remote_main(), before, "origin/main moved");
    assert!(s.gh_log().is_empty(), "{}", s.gh_log());
}

#[test]
fn task_pr_does_not_overwrite_an_existing_unrelated_remote_branch() {
    let s = setup();
    git_out(s.repo.path(), &["checkout", "-q", "-b", "other"]);
    std::fs::write(s.repo.path().join("other.txt"), "x\n").unwrap();
    git_out(s.repo.path(), &["add", "."]);
    git_out(s.repo.path(), &["commit", "-q", "-m", "other work"]);
    git_out(
        s.repo.path(),
        &["push", "-q", "origin", "other:feature/taken"],
    );
    git_out(s.repo.path(), &["checkout", "-q", "main"]);
    let before = git_out(s.remote.path(), &["rev-parse", "feature/taken"]);
    let err = s.pr("feature/taken");
    assert!(err.contains("not part of this task"), "{err}");
    assert_eq!(
        git_out(s.remote.path(), &["rev-parse", "feature/taken"]),
        before
    );
}

#[test]
fn task_pr_surfaces_a_failing_gh_create_and_stores_no_url() {
    let s = setup();
    std::fs::write(s.tools.path().join("fail"), "").unwrap();
    let err = s.pr("feature/pr-file");
    assert!(err.contains("boom from gh"), "{err}");
    let stored = s
        .daemon
        .request("task.get", serde_json::json!({"id": s.id}));
    assert!(stored.get("prUrl").is_none(), "{stored}");
}
