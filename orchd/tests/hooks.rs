//! Black-box integration tests (hooks): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::time::Duration;

#[test]
fn stop_hook_blocks_on_failing_verify_then_task_passes_once_fixed() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(
        scripts_dir.path(),
        "fake-claude-hook.js",
        FAKE_CLAUDE_HOOK_SCRIPT,
    );

    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    // Skip review so this test only exercises the Stop hook + verify gate.
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    // Relative to the worktree (the verify command's cwd), and part of the
    // diff once the fake script writes it -- unlike a file outside the
    // repo, this actually changes the verify-cache key between the hook's
    // call and the post-session gate's.
    let verify_cmd = "test -f verified-marker.txt".to_string();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Stop hook case",
            "goal": "Make a trivial change",
            "criteria": [],
            "verify": [verify_cmd],
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    daemon.request("task.start", serde_json::json!({"id": task_id}));

    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(15));
    assert_eq!(settled["status"], "done", "task JSON: {settled}");
    let attempts = settled["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(
        attempts[0]["gateBlocks"], 1,
        "the one Stop-hook block should be recorded on the attempt"
    );

    let events_path = daemon
        .data_dir()
        .join("tasks")
        .join(&task_id)
        .join("runs")
        .join("1")
        .join("events.jsonl");
    let events_text = std::fs::read_to_string(&events_path).unwrap_or_default();
    assert!(
        events_text.contains("Verification failed"),
        "expected the hook's block reason to show up in events.jsonl: {events_text}"
    );

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

// -- Claude implement run hooks -------------------------------------------

#[test]
fn a_claude_implement_run_gets_only_the_orchd_hooks_and_no_env() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let script = fake_harness_script(scripts_dir.path(), "fake-claude.sh", FAKE_CLAUDE_PASS);
    let args = scripts_dir.path().join("args");
    let daemon = Daemon::spawn(&[
        ("ORCHD_CLAUDE_BIN", script.to_str().unwrap()),
        ("CLAUDE_ARGS", args.to_str().unwrap()),
    ]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["review"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Not lean output",
            "goal": "Change the widget",
            "verify": ["true"],
            "start": true,
        }),
    );
    let task_id = task["id"].as_str().unwrap().to_string();
    let settled = poll_task_status(&daemon, &task_id, Duration::from_secs(20));
    assert_eq!(settled["status"], "done", "{settled}");

    let settings: serde_json::Value =
        serde_json::from_str(&run_file(&daemon, &task_id, "settings.json")).unwrap();
    let hooks: Vec<&String> = settings["hooks"].as_object().unwrap().keys().collect();
    assert_eq!(hooks, ["PreToolUse", "Stop"], "{settings}");
    assert!(settings.get("env").is_none(), "{settings}");

    let worktree = task["worktree"].as_str().unwrap().to_string();
    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}
