//! Black-box integration tests: a final check that already fails on the
//! task's base commit goes to the owner, before any attempt, instead of being
//! retried.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Implements (writes a marker) and, as the reviewer, answers a plain PASS.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

const RETRY: &str = "retry after the base is fixed";

fn start(daemon: &Daemon, repo: &Path, final_check: &str) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Pre-existing",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [final_check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn spawn(scripts: &Path) -> Daemon {
    let script = fake_harness_script(scripts, "fake.sh", FAKE);
    Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())])
}

fn waiting_with_attempts(daemon: &Daemon, id: &str, n: usize) -> Value {
    poll_until(daemon, id, Duration::from_secs(30), |s| s == "waiting");
    let start = std::time::Instant::now();
    loop {
        let task = daemon.request("task.get", json!({"id": id}));
        if task["status"] == "waiting" && task["attempts"].as_array().unwrap().len() == n {
            return task;
        }
        assert!(start.elapsed() < Duration::from_secs(30), "{task}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |t| t.lines().count())
}

#[test]
fn a_final_check_that_fails_twice_on_base_waits_before_any_attempt_and_leaves_no_scratch_dir() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let check = format!("echo ran >> {}; echo boom; exit 3", log.display());
    let id = start(&daemon, repo.path(), &check);

    let task = waiting_with_attempts(&daemon, &id, 0);
    let base = task["baseSha"].as_str().unwrap();
    assert_eq!(
        task["question"]["text"].as_str().unwrap(),
        format!("{check} already fails on base {}: boom", &base[..7])
    );
    assert_eq!(
        task["question"]["options"],
        json!([RETRY, "drop this check", "stop"])
    );
    assert!(
        !task["attempts"][0]["failure"]["kind"].is_string(),
        "{task}"
    );
    assert_eq!(lines(&log), 2, "the base run and its one re-run");

    let listed = git_out(repo.path(), &["worktree", "list"]);
    assert_eq!(listed.lines().count(), 2, "{listed}");
    let scratch = daemon.data_dir().join("base-runs");
    let left = std::fs::read_dir(&scratch).map_or(0, |d| d.count());
    assert_eq!(left, 0, "the temp base dir is gone");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_check_that_passes_on_base_still_retries() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Fails only where the implementer's marker exists.
    let id = start(&daemon, repo.path(), "test ! -f CHANGED_MARKER.txt");
    let task = poll_until(&daemon, &id, Duration::from_secs(60), |s| {
        matches!(s, "waiting" | "failed" | "done")
    });
    assert!(
        task["attempts"].as_array().unwrap().len() > 1,
        "retried as before: {task}"
    );
    assert!(!task["question"]["text"]
        .as_str()
        .unwrap_or("")
        .contains("already fails on base"));
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn drop_this_check_removes_it_and_the_first_attempt_lands() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let check = format!("echo ran >> {}; false", log.display());
    let id = start(&daemon, repo.path(), &check);
    waiting_with_attempts(&daemon, &id, 0);

    daemon.request(
        "task.answer",
        json!({"id": id, "answer": "drop this check"}),
    );
    let done = poll_until(&daemon, &id, Duration::from_secs(30), |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    // An empty list is left out of the JSON.
    assert!(
        done["finalVerify"].as_array().is_none_or(Vec::is_empty),
        "{done}"
    );
    assert!(done["attempts"][0]["failure"].is_null(), "{done}");
    let decisions = done["decisions"].as_array().unwrap();
    assert!(
        decisions.iter().any(
            |d| d.as_str().unwrap().contains(&check) && d.as_str().unwrap().contains("dropped")
        ),
        "{decisions:?}"
    );
    let branch = done["branch"].as_str().unwrap();
    let subject = git_out(repo.path(), &["log", "-1", "--format=%s", branch]);
    assert!(subject.contains("Pre-existing"), "{subject}");
    let listed = git_out(repo.path(), &["worktree", "list"]);
    assert_eq!(listed.lines().count(), 1, "{listed}");

    daemon.shutdown_and_wait();
}

#[test]
fn retry_after_the_base_is_fixed_moves_to_the_new_base_and_runs_the_first_attempt() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let id = start(&daemon, repo.path(), "test -f FIXED.txt");
    let waiting = waiting_with_attempts(&daemon, &id, 0);
    let old_base = waiting["baseSha"].as_str().unwrap().to_string();

    std::fs::write(repo.path().join("FIXED.txt"), "fixed\n").unwrap();
    git_out(repo.path(), &["add", "FIXED.txt"]);
    git_out(repo.path(), &["commit", "-q", "-m", "fix the base"]);
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));

    let done = poll_until(&daemon, &id, Duration::from_secs(60), |s| s == "done");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert_ne!(done["baseSha"].as_str().unwrap(), old_base);
    assert!(
        done["decisions"].as_array().unwrap().iter().any(|d| d
            .as_str()
            .unwrap()
            .starts_with("Rebase: carried the work onto")),
        "{done}"
    );
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn an_unmoved_base_is_asked_about_again_and_re_run_once_past_the_cache() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("ran.log");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let check = format!("echo ran >> {}; false", log.display());
    let id = start(&daemon, repo.path(), &check);
    waiting_with_attempts(&daemon, &id, 0);
    assert_eq!(lines(&log), 2);

    // Nothing moved on the base: the cached failure stands as the first run,
    // the one re-run goes past the cache, and the owner is asked again.
    daemon.request("task.answer", json!({"id": id, "answer": RETRY}));
    let start_wait = std::time::Instant::now();
    while lines(&log) < 3 {
        assert!(start_wait.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(100));
    }
    let again = waiting_with_attempts(&daemon, &id, 0);
    assert_eq!(lines(&log), 3, "{again}");
    assert!(again["question"]["text"]
        .as_str()
        .unwrap()
        .contains("already fails on base"));

    let worktree = again["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_check_that_fails_once_on_the_base_and_then_passes_is_not_asked_about() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    // Exit 1 on its first run anywhere, 0 on every later one: load-flaky.
    let check = format!(
        "n=$(cat {c} 2>/dev/null || echo 0); echo $((n+1)) > {c}; [ \"$n\" -ge 1 ]",
        c = count.display()
    );
    let id = start(&daemon, repo.path(), &check);
    let done = poll_until(&daemon, &id, Duration::from_secs(60), |s| {
        matches!(s, "done" | "waiting" | "failed")
    });
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["attempts"].as_array().unwrap().len(), 1, "{done}");
    assert!(done["question"].is_null(), "{done}");
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn stop_leaves_the_task_stopped() {
    let scripts = tempfile::tempdir().unwrap();
    let daemon = spawn(scripts.path());
    let repo = init_git_repo();
    let id = start(&daemon, repo.path(), "false");
    let waiting = waiting_with_attempts(&daemon, &id, 0);
    daemon.request("task.answer", json!({"id": id, "answer": "stop"}));
    let stopped = daemon.request("task.get", json!({"id": id}));
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    let worktree = waiting["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
