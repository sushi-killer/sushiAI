//! Black-box integration tests (variant.bestOf): spawn the real `orchd` binary
//! with a fake harness and drive it over its NDJSON unix socket.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// One fake plays every role. The implement run writes `a.txt` in the task's
/// worktree and `b.txt` in the sibling `-b` one, and only finishes once the
/// other candidate has started too, so the two must run concurrently.
fn fake_with(verify: &str, checks: &str) -> String {
    format!(
        r###"#!/bin/sh
brief=$(cat)
case "$brief" in
*sushi-plan*)
  printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-plan\n{{\"title\":\"Both\",\"goal\":\"Write a file\",\"tier\":\"hard\",\"criteria\":[\"a file exists\"],\"verify\":[\"{verify}\"],{checks}\"questions\":[]}}\n```"}}'
  ;;
"## Pick"*)
  printf '%s\n' "$brief" > "$LOG_DIR/pick-brief.md"
  printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-pick\n{{\"pick\":\"b\",\"why\":\"b is the tidier change\"}}\n```"}}'
  ;;
*)
  case "$(pwd)" in
    *-b) name=b ;;
    *) name=a ;;
  esac
  touch "$LOG_DIR/started.$name"
  echo "$name" > "$name.txt"
  i=0
  while [ ! -f "$LOG_DIR/started.a" ] || [ ! -f "$LOG_DIR/started.b" ]; do
    i=$((i+1)); [ $i -gt 200 ] && exit 3
    sleep 0.05
  done
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"sess-fake"}}'
  printf '%s\n' '{{"type":"result","total_cost_usd":0.01,"usage":{{"input_tokens":1,"output_tokens":1}},"result":"```sushi-report\n{{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}}\n```"}}'
  ;;
esac
"###
    )
}

fn run(verify: &str, best_of: u32) -> (Value, tempfile::TempDir, tempfile::TempDir, Daemon) {
    run_with(verify, "", best_of, false)
}

fn run_with(
    verify: &str,
    checks: &str,
    best_of: u32,
    grounded: bool,
) -> (Value, tempfile::TempDir, tempfile::TempDir, Daemon) {
    let scripts = tempfile::tempdir().unwrap();
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", &fake_with(verify, checks));
    let log = tempfile::tempdir().unwrap();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", path.to_str().unwrap()),
        ("LOG_DIR", log.path().to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "write a file two ways",
            "variant": {"bestOf": best_of, "bestOfRoute": "claude-sonnet", "groundedChecks": grounded},
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let done = settle(&daemon, &id);
    // Keep the repo and log alive for the assertions.
    let _ = &repo;
    (done, repo, log, daemon)
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn implement(task: &Value) -> Vec<Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .cloned()
        .collect()
}

fn sibling_gone(repo: &Path, task: &Value) {
    let wt = format!("{}-b", task["worktree"].as_str().unwrap());
    assert!(!Path::new(&wt).exists(), "sibling worktree left: {wt}");
    let branch = format!("{}-b", task["branch"].as_str().unwrap());
    let branches = git_out(repo, &["branch", "--list", &branch]);
    assert!(
        branches.trim().is_empty(),
        "sibling branch left: {branches}"
    );
}

fn finish(daemon: Daemon, task: &Value) {
    let wt = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(wt);
}

#[test]
fn when_both_candidates_pass_the_pick_run_decides_and_both_costs_count() {
    let (done, repo, log, daemon) = run("true", 2);
    assert_eq!(done["status"], "done", "{done}");
    let attempts = implement(&done);
    assert_eq!(attempts.len(), 1, "{done}");
    let candidates = attempts[0]["candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{done}");
    assert_eq!(candidates[0]["picked"], false);
    assert_eq!(candidates[1]["picked"], true);
    assert!(candidates.iter().all(|c| c["verify"] == true), "{done}");
    // The pick run's reason is a decision line.
    assert!(
        decisions(&done)
            .iter()
            .any(|d| d.starts_with("Best-of: both passed; picked b")
                && d.contains("b is the tidier change")),
        "{:?}",
        decisions(&done)
    );
    // Two implement runs and one pick run, all counted.
    let cost = done["costUsd"].as_f64().unwrap();
    assert!((cost - 0.04).abs() < 1e-9, "cost {cost}: {done}");
    let pick = std::fs::read_to_string(log.path().join("pick-brief.md")).unwrap();
    assert!(
        pick.contains("## Candidate a") && pick.contains("## Candidate b"),
        "{pick}"
    );

    // The winner's file was committed on the task's branch, the loser's not.
    let branch = done["branch"].as_str().unwrap();
    let tree = git_out(repo.path(), &["ls-tree", "--name-only", branch]);
    assert!(tree.lines().any(|l| l == "b.txt"), "{tree}");
    assert!(!tree.lines().any(|l| l == "a.txt"), "{tree}");
    sibling_gone(repo.path(), &done);
    finish(daemon, &done);
}

#[test]
fn the_only_candidate_that_passes_verify_wins_without_a_pick_run() {
    let (done, repo, log, daemon) = run("test -f b.txt", 2);
    assert_eq!(done["status"], "done", "{done}");
    let attempts = implement(&done);
    assert_eq!(attempts.len(), 1, "{done}");
    let candidates = attempts[0]["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["verify"], false, "{done}");
    assert_eq!(candidates[1]["verify"], true, "{done}");
    assert_eq!(candidates[1]["picked"], true);
    assert!(decisions(&done)
        .iter()
        .any(|d| d.starts_with("Best-of: only b ")));
    assert!(!log.path().join("pick-brief.md").exists());
    let cost = done["costUsd"].as_f64().unwrap();
    assert!((cost - 0.03).abs() < 1e-9, "cost {cost}: {done}");
    sibling_gone(repo.path(), &done);
    finish(daemon, &done);
}

#[test]
fn a_candidate_that_passes_verify_but_fails_a_check_records_verify_true() {
    let checks = r#"\"checks\":[{\"criterion\":0,\"run\":\"test -f b.txt\"}],"#;
    let (done, repo, _log, daemon) = run_with("true", checks, 2, true);
    assert_eq!(done["status"], "done", "{done}");
    let attempts = implement(&done);
    assert_eq!(attempts.len(), 1, "{done}");
    let candidates = attempts[0]["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["verify"], true, "{done}");
    assert_eq!(candidates[0]["checks"]["failed"], 1, "{done}");
    assert_eq!(candidates[1]["verify"], true, "{done}");
    assert_eq!(candidates[1]["checks"]["failed"], 0, "{done}");
    assert_eq!(candidates[1]["picked"], true);
    sibling_gone(repo.path(), &done);
    finish(daemon, &done);
}

#[test]
fn with_best_of_off_a_hard_task_runs_one_candidate() {
    let scripts = tempfile::tempdir().unwrap();
    // Not the concurrent fake: a single run would wait for a sibling forever.
    let path = fake_harness_script(scripts.path(), "fake-claude.sh", FAKE_PLANNER_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", path.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    daemon.request("settings.set", json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "write a file",
            "variant": {"bestOf": 1},
            "start": true,
        }),
    );
    let done = settle(&daemon, task["id"].as_str().unwrap());
    assert_eq!(done["status"], "done", "{done}");
    assert_eq!(done["tier"], "hard");
    let attempts = implement(&done);
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].get("candidates").is_none(), "{done}");
    sibling_gone(repo.path(), &done);
    finish(daemon, &done);
}
