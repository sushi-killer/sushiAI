//! Black-box integration tests (daemon basics): spawn the real `orchd` binary and drive
//! it over its NDJSON unix socket.

mod common;

use common::*;
use std::path::Path;
use std::time::{Duration, Instant};

/// Sends a raw request without going through `request_on` (which panics on
/// an error response) so the "unauthorized" error itself can be asserted on.
fn raw_request(socket: &Path, method: &str, auth: Option<&str>) -> serde_json::Value {
    raw_request_with_params(socket, method, serde_json::json!({}), auth)
}

#[test]
fn requests_without_the_control_token_are_rejected() {
    let daemon = Daemon::spawn(&[]);

    // ping needs no token.
    let ping = request_on(&daemon.socket, "ping", serde_json::json!({}), None);
    assert!(ping["pid"].as_u64().unwrap() > 0);

    // Everything else does: no auth field at all, or the wrong token, both
    // rejected before the method ever runs.
    let no_auth = raw_request(&daemon.socket, "settings.get", None);
    assert_eq!(no_auth["error"]["message"], "unauthorized");
    let wrong_auth = raw_request(&daemon.socket, "settings.get", Some("not-the-real-token"));
    assert_eq!(wrong_auth["error"]["message"], "unauthorized");

    // The real token works.
    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4);

    daemon.shutdown_and_wait();
}

#[test]
fn ping_settings_task_create_and_list_round_trip() {
    let daemon = Daemon::spawn(&[]);

    let ping = daemon.request("ping", serde_json::json!({}));
    assert!(ping["pid"].as_u64().unwrap() > 0);
    assert!(!ping["dataDir"].as_str().unwrap().is_empty());
    assert!(ping["binaryMtimeMs"].as_u64().unwrap() > 0);
    assert_eq!(ping["running"], 0);

    let settings = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(settings["maxAttempts"], 4);
    assert_eq!(settings["tiers"]["standard"], "claude-sonnet");

    let repo = init_git_repo();
    let task = daemon.request(
        "task.create",
        serde_json::json!({
            "repo": repo.path().to_str().unwrap(),
            "title": "Add a widget",
            "goal": "Add a widget to the page",
            "criteria": ["widget visible"],
            "verify": ["true"],
        }),
    );
    assert_eq!(task["status"], "queued");
    assert_eq!(task["branch"], "task/add-a-widget");
    let worktree = task["worktree"].as_str().unwrap();
    assert!(
        Path::new(worktree).is_dir(),
        "worktree directory should exist"
    );

    let list = daemon.request("task.list", serde_json::json!({}));
    let tasks = list.as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["id"], task["id"]);

    let get = daemon.request("task.get", serde_json::json!({"id": task["id"]}));
    assert_eq!(get["title"], "Add a widget");

    daemon.shutdown_and_wait();
    let _ = std::fs::remove_dir_all(worktree);
}

#[test]
fn settings_defaults_reports_the_built_in_defaults_not_the_saved_settings() {
    let daemon = Daemon::spawn(&[]);

    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("claude-sonnet");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let defaults = daemon.request("settings.defaults", serde_json::json!({}));
    assert_eq!(defaults["planner"], "claude-opus");
    let saved = daemon.request("settings.get", serde_json::json!({}));
    assert_eq!(saved["planner"], "claude-sonnet");

    daemon.shutdown_and_wait();
}

/// Regression test for the bug where `App::shutdown()` and `main.rs`'s
/// drain loop only ever looked at `controls` (task attempts), never
/// `chat_turns`: a `shutdown` mid-conversation left the orchestrator chat's
/// `setsid`'d harness child running forever, orphaned, with the daemon gone
/// and nothing left to kill it.
#[test]
fn shutdown_kills_a_live_orchestrator_chat_turn_s_child() {
    let scripts_dir = tempfile::tempdir().unwrap();
    let marker = scripts_dir.path().join("chat-pid");
    // Records its own pid (== the pid tokio spawned, since a shebang script
    // is exec'd in place, never forked again) then hangs without ever
    // printing a `result` line -- exactly what a slow chat turn looks like
    // from orchd's side. The rename keeps the test from reading the marker
    // between its creation and the pid landing in it.
    let body = format!(
        "#!/bin/sh\ncat > /dev/null\necho $$ > {m}.tmp\nmv {m}.tmp {m}\nsleep 30\n",
        m = marker.display()
    );
    let script = fake_harness_script(scripts_dir.path(), "fake-claude-chat.sh", &body);
    let daemon = Daemon::spawn(&[("ORCHD_CLAUDE_BIN", script.to_str().unwrap())]);

    let repo = tempfile::tempdir().unwrap();
    daemon.request(
        "chat.send",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "text": "hello"}),
    );

    let start = Instant::now();
    while !marker.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "chat harness never started"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let pid: i32 = std::fs::read_to_string(&marker)
        .unwrap()
        .trim()
        .parse()
        .expect("marker should contain the harness pid");
    assert!(
        is_alive(pid),
        "the fake harness should still be running before shutdown"
    );

    // Same call the acceptance criteria names: `shutdown` mid-conversation.
    daemon.shutdown_and_wait();

    assert!(
        !is_alive(pid),
        "chat turn's child (pid {pid}) leaked past the daemon's own shutdown"
    );
}

#[test]
fn second_daemon_with_same_data_dir_but_different_socket_refuses_to_start() {
    // Electron can legitimately pick a different `--socket` across launches
    // for the same `--data` dir (it falls back to a `$TMPDIR` path when the
    // natural one is too long), so the singleton lock must not depend on
    // both invocations agreeing on the socket path.
    let data_dir = tempfile::tempdir().unwrap();
    let socket1 = data_dir.path().join("first.sock");
    let first = spawn_orchd_raw(data_dir.path(), &socket1, &[]);
    wait_for_socket(&socket1);

    let socket2 = data_dir.path().join("second.sock");
    let second = spawn_orchd_raw(data_dir.path(), &socket2, &[]);
    let (status, stderr) = wait_for_exit(second, Duration::from_secs(5));
    assert!(!status.success(), "second daemon should refuse to start");
    assert!(
        stderr.contains("already running"),
        "stderr should explain why: {stderr}"
    );
    assert!(
        !socket2.exists(),
        "the second daemon must not have bound a socket of its own"
    );

    // The first daemon must be completely unaffected.
    let ping = request_on(&socket1, "ping", serde_json::json!({}), None);
    assert!(ping["pid"].as_u64().unwrap() > 0);
    let token = read_control_token(data_dir.path());
    let _ = request_on(&socket1, "shutdown", serde_json::json!({}), Some(&token));
    let _ = wait_for_exit(first, Duration::from_secs(5));
}

#[test]
fn daemon_exits_nonzero_when_the_socket_path_is_too_long() {
    let data_dir = tempfile::tempdir().unwrap();
    // Unix socket paths are capped well under 200 bytes on every platform
    // orchd targets; this is comfortably past that so `UnixListener::bind`
    // fails with ENAMETOOLONG.
    let long_name = "x".repeat(200);
    let socket = data_dir.path().join(format!("{long_name}.sock"));
    let child = spawn_orchd_raw(data_dir.path(), &socket, &[]);
    let (status, stderr) = wait_for_exit(child, Duration::from_secs(5));
    assert!(
        !status.success(),
        "daemon should exit non-zero when it can't bind its socket"
    );
    assert!(
        !stderr.trim().is_empty(),
        "daemon should print a clear message when bind fails"
    );
}

#[test]
fn task_create_rejects_request_form_when_the_planner_is_disabled() {
    let daemon = Daemon::spawn(&[]);
    let mut settings = daemon.request("settings.get", serde_json::json!({}));
    settings["planner"] = serde_json::json!("");
    daemon.request("settings.set", serde_json::json!({"settings": settings}));

    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap(), "request": "add a widget"}),
        Some(&daemon.token),
    );
    assert!(
        result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("planner is disabled"),
        "{result}"
    );

    daemon.shutdown_and_wait();
}

#[test]
fn task_create_rejects_neither_request_nor_title_and_goal() {
    let daemon = Daemon::spawn(&[]);
    let repo = init_git_repo();
    let result = raw_request_with_params(
        &daemon.socket,
        "task.create",
        serde_json::json!({"repo": repo.path().to_str().unwrap()}),
        Some(&daemon.token),
    );
    assert!(result.get("error").is_some(), "{result}");

    daemon.shutdown_and_wait();
}
