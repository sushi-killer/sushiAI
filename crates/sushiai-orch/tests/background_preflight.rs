//! Black-box integration tests: attempt 1 starts at once while the base is
//! checked in the background, a finalVerify that fails on the base still
//! gates landing, and the plan lint moves whole-repo suites to finalVerify.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Logs the start of every implement run to `$ORDER_LOG`, writes a marker and
/// answers the reviewer with a plain PASS.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
emit() {
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' "{\"type\":\"result\",\"total_cost_usd\":0.01,\"usage\":{\"input_tokens\":1,\"output_tokens\":1},\"result\":$1}"
}
case "$brief" in
"## Review"*)
  emit '"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"' ;;
*"You check a coding task brief"*)
  emit '"{\"contradiction\": false, \"conflict\": \"\"}"' ;;
*"You judge"*)
  emit '"{\"verdict\": \"feasible\", \"reason\": \"ok\"}"' ;;
*)
  echo implement-start >> "$ORDER_LOG"
  echo changed > CHANGED_MARKER.txt
  emit '"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"' ;;
esac
"###;

fn start(daemon: &Daemon, repo: &Path, verify: &str, final_check: &str) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["briefCheckRoute"] = json!("claude-sonnet");
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Background preflight",
            "goal": "Write the marker",
            "verify": [verify],
            "finalVerify": [final_check],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn spawn(scripts: &Path, order_log: &Path) -> Daemon {
    let script = fake_harness_script(scripts, "fake.sh", FAKE);
    Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("ORDER_LOG", order_log.to_str().unwrap()),
    ])
}

fn log_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

#[test]
fn attempt_one_starts_before_the_slow_base_check_ends() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("order.log");
    let daemon = spawn(scripts.path(), &log);
    let repo = init_git_repo();
    // A verify command that is slow only in the base checkout, fast on the
    // worktree: the base preflight runs it in the background.
    let check = format!(
        "case \"$PWD\" in */base-runs/*) sleep 4;; esac; echo check-end:$PWD >> {}",
        log.display()
    );
    let id = start(&daemon, repo.path(), &check, "true");
    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "failed" | "waiting"));
    assert_eq!(done["status"], "done", "{done}");

    let lines = log_lines(&log);
    let implement = lines.iter().position(|l| l == "implement-start");
    let base_end = lines
        .iter()
        .position(|l| l.starts_with("check-end:") && l.contains("/base-runs/"));
    let (Some(implement), Some(base_end)) = (implement, base_end) else {
        panic!("both events are logged: {lines:?}");
    };
    assert!(
        implement < base_end,
        "implement started before the base check ended: {lines:?}"
    );
    // Landing waited for the job: the base run ended before the task was done.
    let worktree = done["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn a_slow_final_check_that_fails_on_the_base_still_gates_landing() {
    let scripts = tempfile::tempdir().unwrap();
    let log = scripts.path().join("order.log");
    let daemon = spawn(scripts.path(), &log);
    let repo = init_git_repo();
    let check = "case \"$PWD\" in */base-runs/*) sleep 2;; esac; echo boom; exit 3";
    let head_before = git_out(repo.path(), &["rev-parse", "HEAD"]);
    let id = start(&daemon, repo.path(), "true", check);

    let start_at = std::time::Instant::now();
    let task: Value = loop {
        let t = daemon.request("task.get", json!({"id": id}));
        assert!(
            !matches!(t["status"].as_str(), Some("done" | "failed")),
            "the task must not land: {t}"
        );
        if t["status"] == "waiting" && t["attempts"].as_array().unwrap().len() == 1 {
            break t;
        }
        assert!(start_at.elapsed() < common::deadline(), "{t}");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        task["question"]["text"]
            .as_str()
            .unwrap()
            .contains("already fails on base"),
        "{task}"
    );
    assert_eq!(
        log_lines(&log)
            .iter()
            .filter(|l| *l == "implement-start")
            .count(),
        1,
        "attempt 1 ran"
    );
    assert_eq!(
        git_out(repo.path(), &["rev-parse", "HEAD"]),
        head_before,
        "the base branch is unchanged"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn whole_repo_suites_in_a_planned_verify_move_to_final_verify() {
    let scripts = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts.path(), "planner.sh", PLANNER);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["answerPolicy"] = json!(false);
    daemon.request("settings.set", json!({"settings": settings}));

    let repo = init_git_repo();
    std::fs::write(
        repo.path().join("package.json"),
        r#"{"scripts":{"ci":"echo ci","test:daemon":"cargo fmt --check && cargo test --workspace"}}"#,
    )
    .unwrap();
    git_out(repo.path(), &["add", "package.json"]);
    git_out(repo.path(), &["commit", "-q", "-m", "package.json"]);

    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "request": "tidy the daemon",
            "start": false,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();
    let planned = poll_until(&daemon, &id, |s| {
        matches!(s, "stopped" | "done" | "failed" | "waiting")
    });
    assert_eq!(planned["status"], "stopped", "{planned}");

    let list = |field: &str| -> Vec<String> {
        planned[field]
            .as_array()
            .map(|a| a.iter().map(|c| c.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    };
    assert_eq!(list("verify"), vec!["true"], "{planned}");
    assert_eq!(
        list("finalVerify"),
        vec!["npm run ci", "npm run test:daemon"],
        "{planned}"
    );
    let decisions = list("decisions");
    for cmd in ["npm run ci", "npm run test:daemon"] {
        let line =
            format!("Orchestrator: verify entry {cmd} is a whole-repo suite; moved to finalVerify");
        assert!(decisions.contains(&line), "{decisions:?}");
    }

    let worktree = planned["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

const PLANNER: &str = r#"#!/bin/sh
cat > /dev/null
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-plan"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-plan\n{\"title\":\"Tidy\",\"goal\":\"Tidy the daemon\",\"tier\":\"hard\",\"criteria\":[\"Tidy\"],\"verify\":[\"true\",\"npm run ci\",\"npm run test:daemon\"],\"questions\":[]}\n```"}'
"#;
