//! Black-box integration tests: a `verify` command that fails after the work
//! was carried onto a base that moved during the attempt is blamed on the
//! base, not on the attempt, when it fails on that base alone too.

mod common;

use common::*;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

/// Implements (writes a marker) and, the first time only, commits `$NEWFILE`
/// to the base branch in `$REPO` while it works: the base moves under the
/// attempt.
const FAKE: &str = r###"#!/bin/sh
cat > /dev/null
echo changed > CHANGED_MARKER.txt
if [ ! -f "$MOVED" ]; then
  touch "$MOVED"
  (cd "$REPO" && printf x > "$NEWFILE" && git add "$NEWFILE" && git commit -q -m "base moves")
fi
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"```sushi-report\n{\"outcome\":\"complete\",\"summary\":\"done\",\"decisions\":[],\"question\":\"\"}\n```"}'
"###;

struct Setup {
    daemon: Daemon,
    repo: tempfile::TempDir,
    _scratch: tempfile::TempDir,
}

/// A daemon whose harness moves the base by committing `newfile`.
fn setup(newfile: &str) -> Setup {
    let scratch = tempfile::tempdir().unwrap();
    let repo = init_git_repo();
    let script = fake_harness_script(scratch.path(), "fake.sh", FAKE);
    let moved = scratch.path().join("moved");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("REPO", repo.path().to_str().unwrap()),
        ("MOVED", moved.to_str().unwrap()),
        ("NEWFILE", newfile),
    ]);
    let mut settings = daemon.request("settings.get", json!({}));
    settings["review"] = json!("");
    settings["sandbox"] = json!("host");
    daemon.request("settings.set", json!({"settings": settings}));
    Setup {
        daemon,
        repo,
        _scratch: scratch,
    }
}

fn start(s: &Setup, verify: &str) -> String {
    let task = s.daemon.request(
        "task.create",
        json!({
            "repo": s.repo.path().to_str().unwrap(),
            "title": "Moving base",
            "goal": "Write the marker",
            "verify": [verify],
            "land": false,
            "start": true,
        }),
    );
    task["id"].as_str().unwrap().to_string()
}

fn decisions(task: &Value) -> String {
    task["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn implements(task: &Value) -> Vec<&Value> {
    task["attempts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["stage"] == "implement")
        .collect()
}

fn cleanup(s: Setup, task: &Value) {
    let worktree = task["worktree"].as_str().unwrap().to_string();
    s.daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(Path::new(&worktree));
}

#[test]
fn a_command_the_new_base_broke_waits_for_the_owner_without_costing_an_attempt() {
    let s = setup("broken.txt");
    let id = start(&s, "test ! -f broken.txt");
    let task = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| {
        matches!(st, "waiting" | "failed" | "done")
    });
    assert_eq!(task["status"], "waiting", "{task}");
    let attempts = implements(&task);
    assert_eq!(attempts.len(), 1, "no second attempt was started: {task}");
    let failure = &attempts[0]["failure"];
    assert_eq!(failure["kind"], "verify", "{task}");
    assert!(
        failure["signature"].as_str().unwrap().starts_with("base:"),
        "{task}"
    );
    assert!(
        failure["detail"]
            .as_str()
            .unwrap()
            .contains("Do not fix it as part of this task"),
        "{task}"
    );
    let base = task["baseSha"].as_str().unwrap();
    assert!(
        decisions(&task).contains(&format!(
            "Orchestrator: test ! -f broken.txt fails on the base {} too",
            &base[..8]
        )),
        "{task}"
    );
    let question = task["question"]["text"].as_str().unwrap();
    assert!(question.contains("already fails on base"), "{task}");

    // Dropping the check lets the same attempt finish: it never failed.
    s.daemon.request(
        "task.answer",
        json!({"id": id, "answer": "drop this check"}),
    );
    let done = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| {
        matches!(st, "failed" | "done")
    });
    assert_eq!(done["status"], "done", "{done}");
    let attempts = implements(&done);
    assert_eq!(attempts.len(), 1, "{done}");
    assert!(attempts[0]["failure"].is_null(), "{done}");
    assert_eq!(done["verify"], json!([]), "{done}");
    cleanup(s, &done);
}

#[test]
fn a_command_that_passes_on_the_new_base_is_still_the_attempts_failure() {
    let s = setup("harmless.txt");
    // Fails only where the implementer's marker exists, so not on the base.
    let id = start(&s, "test ! -f CHANGED_MARKER.txt");
    let task = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| {
        matches!(st, "waiting" | "failed" | "done")
    });
    let attempts = implements(&task);
    assert!(attempts.len() > 1, "retried as before: {task}");
    let first = &attempts[0]["failure"];
    assert_eq!(first["kind"], "verify", "{task}");
    assert!(
        !first["signature"].as_str().unwrap().starts_with("base:"),
        "{task}"
    );
    assert!(!decisions(&task).contains("fails on the base"), "{task}");
    assert!(
        !task["question"]["text"]
            .as_str()
            .unwrap_or("")
            .contains("already fails on base"),
        "{task}"
    );
    cleanup(s, &task);
}

#[test]
fn a_command_that_already_failed_on_the_old_base_is_not_blamed_on_the_new_one() {
    let s = setup("harmless.txt");
    // The check the work is meant to turn green: absent on every base.
    let id = start(&s, "test -f never-written.txt");
    let task = poll_until(&s.daemon, &id, Duration::from_secs(60), |st| {
        matches!(st, "waiting" | "failed" | "done")
    });
    assert!(implements(&task).len() > 1, "counted as an attempt: {task}");
    assert!(!decisions(&task).contains("fails on the base"), "{task}");
    cleanup(s, &task);
}
