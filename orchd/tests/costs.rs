//! Black-box integration tests (spend analytics): spawn the real `orchd`
//! binary and drive it over its NDJSON unix socket and its CLI.

mod common;

use common::*;
use std::time::Duration;

/// One fake for every role: the plan reply asks for a verify that only the
/// second implement attempt satisfies, so the task plans, implements twice,
/// gets advised in between and reviewed at the end.
const FAKE_FULL_TASK_SCRIPT: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.05,"usage":{"input_tokens":4,"cache_read_input_tokens":6,"output_tokens":2},"result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*"An implement attempt at this task failed"*)
  printf '%s\n' '{"type":"result","total_cost_usd":0.03,"usage":{"input_tokens":1,"output_tokens":1},"result":"Create SECOND."}' ;;
*sushi-plan*)
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.02,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Two tries\",\"goal\":\"Needs a second attempt\",\"criteria\":[\"SECOND exists\"],\"verify\":[\"test -f SECOND\"],\"questions\":[]}\n```"}' ;;
*)
  if [ -f FIRST ]; then echo x > SECOND; else echo x > FIRST; fi
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

fn records(data: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(data.join("costs.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn a_full_task_leaves_one_cost_record_per_run_and_summaries_add_up() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-full.sh", FAKE_FULL_TASK_SCRIPT);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    // The same fake reviews (`auto` would pick a route on the other harness).
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("claude-opus");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));
    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "add a second file",
            "variant": {"advisor": true},
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_until(&daemon, &task_id, Duration::from_secs(30), |s| {
        s == "done" || s == "failed" || s == "stopped" || s == "waiting"
    });
    assert_eq!(settled["status"], "done", "{settled}");

    let all = records(daemon.data_dir());
    let mut runs: Vec<(String, f64)> = all
        .iter()
        .map(|r| {
            assert_eq!(r["taskId"], task_id.as_str());
            assert_eq!(r["repo"], settled["repo"]);
            assert_eq!(r["estimated"], false);
            (
                r["stage"].as_str().unwrap().to_string(),
                r["costUsd"].as_f64().unwrap(),
            )
        })
        .collect();
    runs.sort_by(|a, b| a.0.cmp(&b.0));
    let stages: Vec<&str> = runs.iter().map(|r| r.0.as_str()).collect();
    assert_eq!(
        stages,
        ["advisor", "implement", "implement", "plan", "review"],
        "{all:?}"
    );
    let cost = |stage: &str| runs.iter().find(|r| r.0 == stage).unwrap().1;
    assert!((cost("plan") - 0.02).abs() < 1e-9);
    assert!((cost("advisor") - 0.03).abs() < 1e-9);
    assert!((cost("review") - 0.05).abs() < 1e-9);
    let review = all.iter().find(|r| r["stage"] == "review").unwrap();
    assert_eq!(review["inputTokens"], 4);
    assert_eq!(review["cachedTokens"], 6);
    assert_eq!(review["outputTokens"], 2);
    assert_eq!(review["harness"], "claude");

    // costs.summary and task.get name every stage; the total is the task's.
    let summary = daemon.request(
        "costs.summary",
        serde_json::json!({"groupBy": ["stage"], "sinceDays": 7}),
    );
    let total = summary["totals"]["costUsd"].as_f64().unwrap();
    assert!((total - settled["costUsd"].as_f64().unwrap()).abs() < 1e-9);
    assert_eq!(summary["totals"]["runs"], 5);
    assert_eq!(summary["rows"].as_array().unwrap().len(), 4);
    let detail = daemon.request("task.get", serde_json::json!({"id": task_id}));
    let by_stage = detail["costByStage"].as_array().unwrap();
    assert_eq!(by_stage.len(), 4, "{detail}");

    // The CLI prints the same rows.
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_orchd"))
        .args(["costs", "--json", "--by", "stage", "--data"])
        .arg(daemon.data_dir())
        .output()
        .unwrap();
    let cli: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli["totals"]["runs"], 5);

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
