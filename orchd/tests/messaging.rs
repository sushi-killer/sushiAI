//! Black-box integration tests (messaging): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::{Duration, Instant};

/// Records each attempt's brief under `$MSG_DIR` and holds the first attempt
/// open until the test creates `$MSG_DIR/release`; only the second attempt
/// writes the file verify looks for.
const HOLD_FIRST_ATTEMPT_SCRIPT: &str = r#"#!/bin/sh
input="$(cat)"
if [ ! -f "$MSG_DIR/first.md" ]; then
  printf '%s' "$input" > "$MSG_DIR/first.md"
  while [ ! -f "$MSG_DIR/release" ]; do sleep 0.1; done
else
  printf '%s' "$input" > "$MSG_DIR/second.md"
  echo pass > PASS.txt
fi
echo changed > CHANGED_MARKER.txt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"sess-fake"}'
printf '%s\n' '{"type":"result","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1},"result":"done"}'
"#;

#[test]
fn a_message_reaches_a_running_task_with_its_next_attempt_only() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude.sh",
        HOLD_FIRST_ATTEMPT_SCRIPT,
    );
    let msg_dir = scripts_dir.path().to_str().unwrap().to_string();
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("MSG_DIR", msg_dir.as_str()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let repo_path = repo.path().to_str().unwrap();
    let sender = daemon.request(
        "task.create",
        serde_json::json!({"repo": repo_path, "title": "Sender", "goal": "g", "start": false}),
    );
    let receiver = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo_path,
            "title": "Receiver",
            "goal": "g",
            "verify": ["test -f PASS.txt"],
            "start": true,
        }),
    );
    let (from, to) = (&sender["id"], &receiver["id"]);
    let first = scripts_dir.path().join("first.md");
    let start = Instant::now();
    while !first.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "the first attempt never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // Sent while attempt 1 runs: kept, not delivered, attempt untouched.
    let note = serde_json::json!({"from": from, "to": to, "text": "PEER-NOTE-42"});
    let sent = daemon.request("message.send", note);
    assert_eq!(sent["delivered"], false);
    let inbox = daemon.request("message.inbox", serde_json::json!({"id": to}));
    assert_eq!(inbox[0]["delivered"], false);
    let running = daemon.request("task.get", serde_json::json!({"id": to}));
    assert_eq!(running["status"], "running", "{running}");
    assert_eq!(running["attempts"].as_array().unwrap().len(), 1);
    // The script creates the file before `cat` has written the whole brief.
    let mut first_brief = std::fs::read_to_string(&first).unwrap();
    while !first_brief.contains("## Report format") {
        assert!(start.elapsed() < Duration::from_secs(15), "{first_brief}");
        std::thread::sleep(Duration::from_millis(20));
        first_brief = std::fs::read_to_string(&first).unwrap();
    }
    assert!(first_brief.contains("Your task id is"));
    assert!(!first_brief.contains("PEER-NOTE-42"));

    std::fs::write(scripts_dir.path().join("release"), "").unwrap();
    let settled = poll_task_status(&daemon, to.as_str().unwrap(), Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2, "{settled}");
    // Attempt 1 ran to its own end (verify failed): never stopped or rerun.
    assert_eq!(attempts[0]["failure"]["kind"], "verify");
    let second_brief = std::fs::read_to_string(scripts_dir.path().join("second.md")).unwrap();
    assert!(second_brief.contains("PEER-NOTE-42"), "{second_brief}");
    let inbox = daemon.request("message.inbox", serde_json::json!({"id": to}));
    assert_eq!(inbox[0]["delivered"], true);

    daemon.shutdown_and_wait();
    for t in [&sender, &receiver] {
        let _ = std::fs::remove_dir_all(t["worktree"].as_str().unwrap());
    }
}
