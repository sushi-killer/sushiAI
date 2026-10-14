//! Black-box tests of the "whose failure" turn: a final check that fails on
//! the base too, with the same failing tests but other output that changed,
//! is read by one agent turn. The fake harness answers that turn with a
//! canned reply and counts how often it is asked.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;

/// Implements (writes a marker), reviews with PASS and, as the attribution
/// turn, counts the call and answers with `REPLY`.
const FAKE: &str = r###"#!/bin/sh
brief=$(cat)
case "$brief" in
"## Review"*)
  printf '%s\n' '{"type":"result","result":"```sushi-review\n{\"verdict\":\"PASS\",\"findings\":[]}\n```"}' ;;
*"sushi-attribution"*)
  echo asked >> COUNT
  printf '%s\n' '{"type":"result","result":"```sushi-attribution\nREPLY\n```"}' ;;
*)
  echo changed > CHANGED_MARKER.txt
  printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
  printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}' ;;
esac
"###;

/// The same failing test on both sides, and other output that differs.
const CHECK: &str = "printf 'not ok 1 - alpha\\n'; case \"$PWD\" in */base-runs/*) echo 'ERROR connecting to db: refused';; *) echo 'ERROR connecting to db: timeout';; esac; exit 1";

fn spawn(scripts: &Path, count: &Path, reply: &str) -> Daemon {
    let body = FAKE
        .replace("COUNT", &count.display().to_string())
        .replace("REPLY", reply);
    let script = fake_harness_script(scripts, "fake.sh", &body);
    Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())])
}

fn start(daemon: &Daemon, repo: &Path) -> String {
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.to_str().unwrap(),
            "title": "Red base",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [CHECK],
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn decisions(task: &Value) -> Vec<String> {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d.as_str().unwrap().to_string())
        .collect()
}

fn asked_about_the_base(task: &Value) -> bool {
    task["question"]["kind"] == "preexisting_failure"
        || task["questionHistory"]
            .as_array()
            .is_some_and(|h| h.iter().any(|q| q["kind"] == "preexisting_failure"))
}

fn finish(daemon: Daemon, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    remove_dir_retrying(worktree);
}

fn turns(count: &Path) -> usize {
    std::fs::read_to_string(count).map_or(0, |t| t.lines().count())
}

#[test]
fn an_inherited_verdict_whose_names_are_in_the_base_run_waives_without_asking() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    let reply = r#"{\"cause\":\"inherited\",\"pre_existing\":[\"alpha\"],\"evidence\":\"alpha fails on the base\"}"#;
    let daemon = spawn(scripts.path(), &count, reply);
    let repo = init_git_repo();
    let id = start(&daemon, repo.path());

    let done = poll_until(&daemon, &id, |s| matches!(s, "done" | "waiting" | "failed"));
    assert_eq!(done["status"], "done", "{done}");
    assert!(!asked_about_the_base(&done), "{done}");
    assert!(
        decisions(&done)
            .iter()
            .any(|d| d.contains("base failures on") && d.contains("alpha")),
        "{:?}",
        decisions(&done)
    );
    let assumed = done["assumptions"].as_array().unwrap();
    assert_eq!(assumed.len(), 1, "{done}");
    assert_eq!(assumed[0]["by"], "judge", "{done}");
    assert_eq!(done["attempts"][0]["cause"], "inherited", "{done}");
    assert_eq!(turns(&count), 1);
    finish(daemon, &done);
}

#[test]
fn a_claim_in_the_output_with_a_name_the_base_lacks_is_not_waived_and_the_verdict_is_cached() {
    let scripts = tempfile::tempdir().unwrap();
    let count = scripts.path().join("count");
    // The agent calls `gamma` pre-existing, but the base run never names it.
    let reply = r#"{\"cause\":\"inherited\",\"pre_existing\":[\"gamma\"],\"evidence\":\"the output says this failure is pre-existing\"}"#;
    let daemon = spawn(scripts.path(), &count, reply);
    let repo = init_git_repo();
    let check = "printf 'not ok 1 - alpha\\n'; case \"$PWD\" in */base-runs/*) echo 'ERROR connecting to db: refused';; *) echo 'this failure is pre-existing: gamma';; esac; exit 1";
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("claude-opus");
    daemon.request("settings.set", json!({"settings": settings}));
    let task = daemon.request(
        "task.create",
        json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Red base",
            "goal": "Write the marker",
            "verify": ["true"],
            "finalVerify": [check],
            "start": true,
        }),
    );
    let id = task["id"].as_str().unwrap().to_string();

    // The second attempt meets the same evidence and fails the same way.
    let task = wait_until("a second failed attempt", || {
        let task = daemon.request("task.get", json!({"id": id}));
        task["attempts"][1]["failure"].is_object().then_some(task)
    });
    assert_eq!(task["attempts"][0]["failure"]["kind"], "verify", "{task}");
    assert_eq!(task["attempts"][0]["cause"], "introduced", "{task}");
    assert!(!asked_about_the_base(&task), "{task}");
    assert!(
        task["assumptions"].as_array().is_none_or(|a| a.is_empty()),
        "{task}"
    );
    assert!(
        decisions(&task)
            .iter()
            .all(|d| !d.contains("base failures on")),
        "{:?}",
        decisions(&task)
    );
    // Every attempt met the same evidence: one turn.
    assert_eq!(turns(&count), 1);
    finish(daemon, &task);
}
