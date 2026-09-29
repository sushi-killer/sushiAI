//! Black-box integration tests (landing queue): a `land` task carries its
//! work onto its base branch by itself. Real daemon, real git repo.

mod common;

use common::*;
use std::path::Path;
use std::time::Duration;

/// Writes FILE_<name>.txt for the task whose goal names `FILE_<name>`; a
/// conflicted `shared.txt` is resolved by keeping both sides.
const LAND_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
if grep -q '<<<<<<<' shared.txt 2>/dev/null; then printf 'a\nb\n' > shared.txt; name=; fi
name=${name-$(printf '%s' "$input" | grep -o 'FILE_[a-z]*' | head -1 | sed 's/FILE_//')}
if [ -z "$name" ]; then :
elif [ "$name" = shared ]; then echo b > shared.txt
else
  echo "$name" > "$name.txt"
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"#;

fn daemon() -> (Daemon, tempfile::TempDir) {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "fake-claude.sh", LAND_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    (daemon, scripts)
}

/// A repo with the work branch `work` checked out (the default branch is
/// left alone).
fn repo_on_work_branch() -> tempfile::TempDir {
    let repo = init_git_repo();
    git_out(repo.path(), &["checkout", "-q", "-b", "work"]);
    repo
}

fn create(daemon: &Daemon, repo: &Path, name: &str, verify: &str, land: bool) -> serde_json::Value {
    daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.to_str().unwrap(),
            "title": format!("Add {name}"),
            "goal": format!("FILE_{name}: write the file"),
            "verify": [verify],
            "land": land,
            "start": false,
        }),
    )
}

fn until_done(daemon: &Daemon, id: &str) -> serde_json::Value {
    poll_until(daemon, id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "failed" | "stopped")
    })
}

fn decisions(task: &serde_json::Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn commits(repo: &Path, branch: &str) -> Vec<String> {
    git_out(repo, &["log", "--format=%s", branch])
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn two_land_tasks_land_one_after_another_as_single_commits() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let one = create(&daemon, root, "one", "test -f one.txt", true);
    let two = create(&daemon, root, "two", "test -f two.txt", true);
    let (one_id, two_id) = (
        one["id"].as_str().unwrap().to_string(),
        two["id"].as_str().unwrap().to_string(),
    );
    daemon.request("task.start", serde_json::json!({"id": one_id}));
    daemon.request("task.start", serde_json::json!({"id": two_id}));
    let one = until_done(&daemon, &one_id);
    let two = until_done(&daemon, &two_id);
    assert_eq!(one["status"], "done", "{one}");
    assert_eq!(two["status"], "done", "{two}");

    let head = git_out(root, &["rev-parse", "work"]);
    let landed = [
        one["landedSha"].as_str().unwrap(),
        two["landedSha"].as_str().unwrap(),
    ];
    assert_ne!(landed[0], landed[1]);
    assert!(
        landed.contains(&head.as_str()),
        "head {head}, landed {landed:?}"
    );
    let log = commits(root, "work");
    assert_eq!(log.len(), 3, "{log:?}");
    assert!(log.contains(&"Add one".to_string()) && log.contains(&"Add two".to_string()));
    // The checkout of the base branch holds both files.
    assert!(root.join("one.txt").exists() && root.join("two.txt").exists());
    assert_eq!(git_out(root, &["status", "--porcelain", "-uno"]), "");
    assert!(decisions(&one).contains("Land: landed on work"), "{one}");
    daemon.shutdown_and_wait();
}

#[test]
fn a_conflicting_land_task_fails_an_attempt_and_lands_after_the_next_passing_one() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let verify = "grep -q b shared.txt && ! grep -q '<<<<<<<' shared.txt";
    let second = create(&daemon, root, "shared", verify, true);
    let second_id = second["id"].as_str().unwrap().to_string();
    // The earlier landing puts a different shared.txt on the branch.
    std::fs::write(root.join("shared.txt"), "a\n").unwrap();
    git_out(root, &["add", "shared.txt"]);
    git_out(root, &["commit", "-q", "-m", "earlier landing"]);
    daemon.request("task.start", serde_json::json!({"id": second_id}));
    let second = until_done(&daemon, &second_id);
    assert_eq!(second["status"], "done", "{second}");
    let attempts = second["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{second}");
    assert_eq!(attempts[0]["status"], "failed");
    assert!(
        attempts[0]["failure"]["detail"]
            .as_str()
            .unwrap()
            .contains("These files conflict: shared.txt"),
        "{second}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("shared.txt")).unwrap(),
        "a\nb\n"
    );
    let log = commits(root, "work");
    assert_eq!(log[0], "Add shared", "{log:?}");
    assert_eq!(log[1], "earlier landing");
    assert_eq!(second["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}

#[test]
fn a_dirty_base_checkout_makes_the_task_wait_landing_and_is_left_alone() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    std::fs::write(root.join("README.md"), "hello\nmy edit\n").unwrap();
    let before = git_out(root, &["rev-parse", "work"]);
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let waiting = poll_until(&daemon, &id, Duration::from_secs(60), |s| {
        matches!(s, "landing" | "done" | "failed" | "stopped")
    });
    assert_eq!(waiting["status"], "landing", "{waiting}");
    assert!(
        decisions(&waiting).contains("uncommitted changes"),
        "{waiting}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );
    assert_eq!(git_out(root, &["rev-parse", "work"]), before);

    // A retry with the checkout still dirty changes nothing.
    daemon.request("task.start", serde_json::json!({"id": id}));
    std::thread::sleep(Duration::from_millis(1500));
    let still = daemon.request("task.get", serde_json::json!({"id": id}));
    assert_eq!(still["status"], "landing", "{still}");
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "hello\nmy edit\n"
    );

    git_out(root, &["checkout", "--", "README.md"]);
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    assert!(root.join("one.txt").exists());
    daemon.shutdown_and_wait();
}

#[test]
fn landing_on_the_default_branch_is_refused_unless_land_on_default_is_on() {
    let (daemon, _scripts) = daemon();
    let repo = init_git_repo();
    let root = repo.path();
    let default = git_out(root, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let before = git_out(root, &["rev-parse", "HEAD"]);
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert!(done.get("landedSha").is_none(), "{done}");
    assert!(decisions(&done).contains("Land: refused"), "{done}");
    assert_eq!(git_out(root, &["rev-parse", "HEAD"]), before);
    assert!(!root.join("one.txt").exists());
    let branch = done["branch"].as_str().unwrap();
    assert_eq!(commits(root, branch)[0], "Add one");

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["landOnDefault"] = serde_json::json!(true);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let task = create(&daemon, root, "two", "test -f two.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", &default]));
    assert!(root.join("two.txt").exists());
    daemon.shutdown_and_wait();
}

#[test]
fn after_land_commands_run_in_the_main_checkout_and_a_failure_never_unlands() {
    let (daemon, _scripts) = daemon();
    let repo = repo_on_work_branch();
    let root = repo.path();
    let repo_path = root.to_str().unwrap();
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["afterLand"] = serde_json::json!([
        {"repo": repo_path, "run": "cp one.txt built.txt"},
        {"repo": repo_path, "run": "echo boom >&2; exit 3"},
        {"repo": "/somewhere/else", "run": "exit 9"},
    ]);
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let task = create(&daemon, root, "one", "test -f one.txt", true);
    let id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": id}));
    let done = until_done(&daemon, &id);
    assert_eq!(done["status"], "done", "{done}");
    let lines = decisions(&done);
    assert!(lines.contains("exited 0"), "{lines}");
    assert!(lines.contains("exited 3: boom"), "{lines}");
    assert!(!lines.contains("exit 9"), "{lines}");
    assert!(root.join("built.txt").exists());
    assert_eq!(done["landedSha"], git_out(root, &["rev-parse", "work"]));
    daemon.shutdown_and_wait();
}
